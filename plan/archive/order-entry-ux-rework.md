# Feature: Order Entry UX Rework — Instant Brackets, Bidirectional Sync, Smart Recall

## Overview

Rework the order entry workflow so that clicking BUY or SELL instantly draws dashed bracket lines on the chart (Draft status), number inputs and chart lines stay in bidirectional sync, number inputs support mouse wheel adjustment using the existing `price_step_for()` increment rules, and pressing [X] hides (but persists) the bracket per symbol. When recalled later, brackets that drifted more than 1 G.ATR from the current price are automatically repositioned to the current price level.

This benefits the trader by eliminating the 3-click placement flow for the common case, keeping the panel and chart always in sync, and preventing "lost bracket" confusion when returning to a symbol after the price has moved.

**Assumptions**:
- The existing 3-click `BracketTool` remains available as an alternative entry method (not removed). The two flows do not interact — a user can have both an instant bracket annotation and a 3-click tool in progress simultaneously.
- "Repositioning" means shifting all three legs (entry, TP, SL) by the same delta to center the entry near the current price, preserving the bracket's shape (R:R ratio).
- G.ATR absolute value needs to be exposed from `GatrResult` (currently only percentage is available).
- Hidden brackets are per-symbol. If the user switches symbols while a bracket is hidden, the hidden bracket stays in the old symbol's annotation store. It reappears when the user returns to that symbol.

## Research Summary

### Codebase Analysis

**Existing bracket infrastructure is mature.** `OrderBracket` (`desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs`) has full status-driven styling: Draft = dashed lines (6/4 dash/gap), Pending = dotted, Active = solid. `compute_bracket()` produces all render primitives (lines, labels, hit zones, fills, buttons). The `BracketStatus::Draft` path and the `saved` flag for persistence survival already exist.

**Order panel state and annotation store are linked.** `OrderPanelState` has `bracket_annotation_id: Option<AnnotationId>` and `bracket_active: Option<OrderSide>`. `AnnotationStore` persists per-symbol to `data/annotations/<SYMBOL>.json` via `annotation_persistence.rs`. The `Presence::Hidden` state exists (renders nothing, zero GPU cost, still in storage, serialized correctly to JSON).

**Bidirectional sync is partially built.** The panel has `SetTpValue`, `SetSlValue`, `SetLimitPrice`, `SetStopPrice` actions. Chart drag produces `HitZoneKind::BracketTP`/`BracketSL` drag events in `interaction/mod.rs`. The missing links: (a) when the panel mutates prices, the annotation isn't updated; (b) when chart drag mutates the annotation, the panel string inputs aren't updated.

**Mouse wheel on number inputs has a working pattern.** The level editor (`views.rs:2615-2621`) uses `iced::widget::mouse_area().on_scroll()` wrapping a price row, converting `ScrollDelta` to `Message::LevelEditorPriceStep`. The increment function `price_step_for()` (`levels.rs:90-96`) returns `(coarse, fine)` steps: 5c for prices >= $200, 1c below.

**G.ATR is computed but only exposes percentage.** `GatrResult` (`desktop/win/crates/midas-core/src/atr/mod.rs:58`) has `pct` and `selected_bars`. The absolute ATR value is computed internally as a local variable (`filtered_avg` or `raw_avg` fallback) but not returned. `MarketSnapshot` stores `gatr_pct: Option<f32>` but not the absolute ATR. For the 1-G.ATR distance threshold, we need the absolute value.

**`saved` flag controls [X] behavior.** `OrderBracket.saved` determines what [X] does: unsaved drafts are deleted; saved drafts are hidden via `Presence::Hidden`. The existing `draft_bracket_cache` in app.rs uses in-memory storage; this plan transitions to `Presence::Hidden` in the annotation store for proper disk persistence. **The `draft_bracket_cache` field and all `sync_draft_cache()` call sites must be removed** to prevent dual-state bugs — the annotation store becomes the sole persistence mechanism for Draft brackets.

**Price snapping inconsistency.** Bracket leg drags in `interaction/mod.rs` use `snap_to_tick()` with a fixed `DEFAULT_TICK_SIZE`. The level editor uses dynamic `price_step_for()`. V1 accepts this inconsistency; unification is a follow-up.

### Best Practices & Idiomatic Approach

**Single source of truth (Elm Architecture).** Both the panel and chart read/write the same `OrderBracket` in `AnnotationStore`. The panel's string fields (`tp_value`, `sl_value`, etc.) are view-layer formatting of the annotation's `f64` prices. On panel input: parse string → mutate annotation → chart re-renders. On chart drag: mutate annotation → format to string → panel re-renders.

**Mouse wheel via `mouse_area().on_scroll()`.** The level editor demonstrates this pattern. No custom widget needed — wrap each price input row in a `mouse_area` with an `on_scroll` handler that emits the same `SetTpValue`/`SetSlValue` actions with incremented values.

**Dashed line rendering via CPU segmentation.** `LineStyle::Dashed` is defined and `leg_style()` returns it for Draft brackets. The `compute_bracket` function currently ignores the style and draws solid `GridLineInstance` rects. Closing this gap means adding a segmentation helper that splits one full-width rect into N short rects with gaps. This is new code — no existing segmentation pattern to reference.

## Design Decisions

### Decision: Instant bracket placement vs preserving 3-click flow
**Context**: User wants brackets to appear immediately on BUY/SELL click. The 3-click flow is currently the only way to create brackets.
**Options considered**:
1. Replace 3-click with instant bracket — simpler code, one workflow
2. Keep both — instant bracket from panel, 3-click from chart tool
**Recommendation**: Option 2. The `BracketTool` serves a different use case (quick chart-only bracket placement without the panel). The two flows don't interact; they can coexist.
**Confidence**: high

### Decision: Default TP/SL offset for instant brackets
**Context**: When user clicks BUY, a bracket appears instantly. Where do TP and SL default to?
**Options considered**:
1. Fixed percentage (e.g., entry ± 1% for TP, ± 0.5% for SL)
2. Based on G.ATR (e.g., TP at entry + 0.5 ATR, SL at entry - 0.3 ATR)
3. No default TP/SL — just entry line, user drags to add
**Recommendation**: Option 1 for V1 (TP at entry ± 1%, SL at entry ∓ 0.5%). Simple, predictable, easy to adjust. G.ATR-based defaults are a follow-up. If TP/SL are disabled in the panel (`tp_enabled`/`sl_enabled` = false), those legs are omitted.
**Confidence**: medium — user may want G.ATR-based defaults later, but 1% is a sane starting point.

### Decision: Where to store hidden bracket state
**Context**: When [X] is clicked on an unsubmitted bracket, it needs to be hidden but persisted per symbol.
**Options considered**:
1. Set `Presence::Hidden` on the existing annotation — stays in `AnnotationStore`, serializes to JSON
2. Separate "stashed brackets" HashMap outside annotations (current `draft_bracket_cache` approach)
**Recommendation**: Option 1. `Presence::Hidden` already exists, serializes correctly, and the annotation store handles per-symbol persistence. This replaces the in-memory `draft_bracket_cache` with proper disk persistence.
**Confidence**: high

### Decision: Repositioning strategy for recalled brackets
**Context**: When a hidden bracket is recalled and price has moved > 1 G.ATR away, the bracket should be repositioned.
**Options considered**:
1. Shift all legs by the same delta (preserves R:R shape)
2. Reset to defaults (loses user's carefully tuned legs)
3. Shift entry only, recalculate TP/SL from scratch
**Recommendation**: Option 1. Compute `delta = current_price - bracket.entry.price`, apply delta to all legs. This preserves the trader's risk/reward structure.
**Confidence**: high

### Decision: How to expose absolute G.ATR value
**Context**: `GatrResult` only has percentage. Need absolute ATR for the 1-ATR distance check.
**Options considered**:
1. Add `avg_atr: f64` field to `GatrResult` — expose the filtered average
2. Add a separate function
3. Compute it in the app layer independently
**Recommendation**: Option 1. The value is already computed inside `gerchik_gatr_detail()` as a local variable. Adding one field is minimal, and `MarketSnapshot` can carry `gatr_abs: Option<f64>` alongside `gatr_pct`.
**Confidence**: high

## Implementation Plan

### Slice 1: Expose absolute G.ATR value
**Goal**: Make the absolute ATR value available for the repositioning distance check.
**Depends on**: None
**Files to create or modify**:
- `desktop/win/crates/midas-core/src/atr/mod.rs` — Add `avg_atr: f64` field to `GatrResult`, populate from the already-computed filtered average variable inside `gerchik_gatr_detail()`
- `desktop/win/crates/midas-core/src/atr/mod.rs` — Add test in inline `#[cfg(test)]` module asserting `avg_atr` value for known inputs
- `desktop/win/crates/midas-core/src/market_data.rs` — Add `gatr_abs: Option<f64>` field to `MarketSnapshot` with `#[serde(default)]`
- `desktop/win/crates/midas-app/src/market_cache.rs` — Switch `compute_daily_gatr()` from `gerchik_gatr_pct` to `gerchik_gatr_detail` and extract both `pct` and `avg_atr`
**Key implementation details**:
- Inside `gerchik_gatr_detail()`, the filtered average is computed in a local variable (line ~120-130 area: `let avg = sum / selected_bars.len() as f64`). Store it in the returned `GatrResult` as `avg_atr`.
- For the fallback case (all bars paranormal), use `raw_avg`.
- `MarketSnapshot.gatr_abs` uses `#[serde(default)]` for backward compat with existing watchlist data.
- `compute_daily_gatr()` changes return type or is supplemented with a new function returning `(Option<f32>, Option<f64>)` for pct and abs.
**Testing**:
- Unit test: 9 uniform bars with TR=20, assert `avg_atr ≈ 20.0`.
- Existing `gatr_pct_*` tests continue passing unchanged.
**Done when**: `MarketSnapshot.gatr_abs` returns `Some(f64)` for symbols with enough daily data.

### Slice 2: Dashed line rendering for Draft brackets
**Goal**: Draft brackets render with actual dashed lines instead of solid lines.
**Depends on**: None (can run in parallel with Slice 1)
**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — In `compute_bracket()`, use the `LineStyle` returned by `leg_style()` to segment lines when style is `Dashed` or `Dotted`
**Key implementation details**:
- Add a helper function `fn segmented_line(x0: f32, x1: f32, y: f32, height: f32, color: [f32; 4], style: LineStyle) -> Vec<GridLineInstance>` in order_bracket module. **Note**: V1 places this in `order_bracket/mod.rs` for locality. When `HorizontalLevel` gains dash support, promote it to `widget/compute.rs` as a shared utility (acceptable prudent-deliberate debt).
- For `LineStyle::Solid`, return one instance (current behavior). For `Dashed { dash_len, gap_len }`, emit `ceil((x1-x0) / (dash + gap))` short rects. For `Dotted { dot_spacing }`, emit small squares.
- Replace the three `output.lines.push(GridLineInstance { ... })` calls (entry, TP, SL) with `output.lines.extend(segmented_line(...))`.
- Viewport of 2000px at 6/4 dashes = ~200 instances per line. With 3 legs = ~600 per bracket. Trivially fast for GPU (each `GridLineInstance` is ~32 bytes).
**Testing**:
- Unit test: `segmented_line(0.0, 100.0, 50.0, 1.0, color, Dashed { 6.0, 4.0 })` produces 10 instances.
- Unit test: `segmented_line(...)` with `Solid` produces exactly 1 instance.
- Unit test: `segmented_line(...)` with `Dotted { 4.0 }` produces 25 instances for 100px width.
**Done when**: Draft brackets show dashed lines; Pending show dotted; Active/Closed show solid.

### Slice 3: Instant bracket on BUY/SELL click with panel sync
**Goal**: Clicking BUY or SELL immediately creates a Draft bracket on the chart and populates panel inputs from its prices.
**Depends on**: None (soft dependency on Slice 2 for visual correctness; works with solid lines too)
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — In the `SetBracketMode(Some(side))` handler, create a Draft `OrderBracket` annotation in `AnnotationStore`, store the `AnnotationId`, and sync panel inputs
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — Add helper `fn default_bracket_prices(last_price: f64, side: OrderSide, tp_enabled: bool, sl_enabled: bool) -> (f64, Option<f64>, Option<f64>)`
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — Add helper `fn sync_panel_from_bracket(state: &mut OrderPanelState, bracket: &OrderBracket)` that populates all string inputs from bracket f64 prices
**Key implementation details**:
- `default_bracket_prices()`: entry = last_price, TP = entry * 1.01 (Buy) or entry * 0.99 (Sell), SL = entry * 0.995 (Buy) or entry * 1.005 (Sell). Returns `None` for TP/SL if the corresponding `_enabled` flag is false.
- When `SetBracketMode(Some(Buy))` fires:
  1. If `last_price` is `None`, create bracket at entry=0.0. Validation will prevent submission; user can drag entry line or wait for data.
  2. Compute prices via `default_bracket_prices()`.
  3. Create `OrderBracket { status: Draft, saved: false, entry_type, ... }`.
  4. Add to `AnnotationStore` → get `AnnotationId` → store in `panel.state.bracket_annotation_id`.
  5. Call `sync_panel_from_bracket()` to populate `tp_value`, `sl_value`, `limit_price`, `stop_price` strings.
- `sync_panel_from_bracket()` formats each leg price with `format!("{:.2}", price)`, or clears the string if the leg is `None`.
- When `SetBracketMode(None)` (X clicked): defer to Slice 5 hide logic. For now, delete the bracket (existing behavior).
- **Remove `draft_bracket_cache`**: Delete the `draft_bracket_cache: HashMap<(OrderPanelId, String), OrderBracket>` field from `MidasApp` (app.rs:168) and the `sync_draft_cache()` method (app.rs:1087-1114). The compiler will flag all ~15 dead call sites across app.rs. Remove each one — the annotation store via `Presence::Hidden` (Slice 5) replaces this mechanism entirely. This prevents dual-state bugs where brackets could be restored from cache instead of from the store.
**Testing**:
- `default_bracket_prices(100.0, Buy, true, true)` → `(100.0, Some(101.0), Some(99.5))`.
- `default_bracket_prices(100.0, Sell, true, false)` → `(100.0, Some(99.0), None)`.
- `sync_panel_from_bracket` populates tp_value="101.00", sl_value="99.50" from bracket.
- Integration: clicking BUY creates annotation and panel shows correct prices.
**Done when**: Clicking BUY/SELL shows a bracket at current price; panel inputs match the bracket's prices.

### Slice 4: Bidirectional sync — panel inputs ↔ chart bracket
**Goal**: Editing a number in the panel moves the chart line; dragging a chart line updates the panel input.
**Depends on**: Slice 3

**Message flow — Panel → Chart:**
```
User types "192.50" in TP field
  → SetTpValue("192.50") emitted
  → app handler: parse to f64
  → annotation_store.update(symbol, id, |ann| set tp.price = 192.50)
  → dirty flag bumps
  → next frame: compute_bracket() reads updated annotation
  → chart renders line at 192.50
```

**Message flow — Chart → Panel:**
```
User drags TP line to y=192.50
  → interaction.rs: DragBracketTP event
  → app handler: annotation_store.update(symbol, id, |ann| set tp.price = 192.50)
  → app handler: find panel with matching bracket_annotation_id
  → panel.state.tp_value = format!("{:.2}", 192.50)
  → dirty flag bumps
  → next frame: panel renders "192.50" in input
```

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — In `OrderPanelAction::SetTpValue` / `SetSlValue` / `SetLimitPrice` / `SetStopPrice` handlers, parse the input string and update the corresponding `OrderBracket` leg in `AnnotationStore`
- `desktop/win/crates/midas-app/src/app.rs` — In the chart drag handler for `HitZoneKind::BracketTP` / `BracketSL`, after updating the annotation, find any order panel whose `bracket_annotation_id` matches and update its string inputs via `sync_panel_from_bracket()`
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — Verify bracket leg drag emits a `ChartAction` with the annotation ID and new price (should already exist)
**Key implementation details**:
- **Panel → Chart**: When `SetTpValue("192.50")` is processed:
  1. Parse to `f64`. If unparseable, don't update annotation (chart line stays at last valid position). Push `("tp", "Invalid price")` to `panel.state.errors`. On next valid parse, remove any error with key `"tp"` from `panel.state.errors`.
  2. Find annotation via `bracket_annotation_id`.
  3. `annotation_store.update(symbol, id, |ann| { if let AnnotationKind::OrderBracket(b) = &mut ann.kind { if let Some(tp) = b.take_profit.as_mut() { tp.price = parsed_price; } } })`.
- **Chart → Panel**: The drag handler is `Message::ChartDragBracketLeg` at app.rs:3870. It already updates the annotation via `annotation_store.update()`. After that existing update, add:
  1. Find the panel: iterate `order_panels` for one with `bracket_annotation_id == Some(dragged_annotation_id)`.
  2. Call `sync_panel_from_bracket()` to update string inputs.
  The `ChartDragBracketLeg` message is emitted from `chart_widget.rs:1130-1134` when `ChartAction::DragBracketLeg` fires.
- Edge case: user types while dragging — last write wins. Both paths go through `annotation_store.update()` which bumps generation atomically.
**Testing**:
- Setting TP value "192.50" → annotation TP leg becomes 192.50.
- Simulated drag → panel's tp_value string becomes "192.50".
- Round-trip: type 192.50 → annotation updated → panel still shows "192.50" (no drift).
- Unparseable input ("abc") → annotation unchanged, panel shows validation error.
**Done when**: Typing a price moves the chart line; dragging a chart line updates the panel input.

### Slice 5: Hide on [X], persist, and smart recall with G.ATR repositioning
**Goal**: [X] hides brackets per symbol (not deletes), recalls them on BUY/SELL, and repositions if > 1 G.ATR from current price.
**Depends on**: Slice 1, Slice 3

**`saved` flag semantics**: The `OrderBracket.saved` field controls [X] behavior. Unsaved drafts are deleted on [X] (existing behavior). Saved drafts transition to `Presence::Hidden` on [X]. The [Save] button on the bracket entry line sets `saved = true`. This plan's hide/recall logic applies only to saved brackets.

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — In `SetBracketMode(None)` handler: set `Presence::Hidden` for saved brackets instead of deleting. In `SetBracketMode(Some(side))`: check for existing hidden bracket before creating new.
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — Add `fn should_reposition(entry_price: f64, current_price: f64, gatr_abs: Option<f64>) -> bool`
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — Add `fn reposition_bracket(bracket: &mut OrderBracket, current_price: f64)`
**Key implementation details**:
- **Hide** (`SetBracketMode(None)` when `bracket_annotation_id.is_some()` and bracket is Draft):
  1. If `bracket.saved`: `annotation_store.update(symbol, id, |ann| ann.presence = Presence::Hidden)`.
  2. If `!bracket.saved`: `annotation_store.remove(symbol, id)` (existing behavior).
  3. Clear `panel.state.bracket_active` to `None`.
  4. Keep `bracket_annotation_id` set (for recall of saved brackets) or clear it (for deleted unsaved ones).
- **Recall** (`SetBracketMode(Some(side))`):
  1. If `bracket_annotation_id` points to a hidden saved bracket in `AnnotationStore`:
     a. If the recalled bracket's side matches the requested side, unhide it: `ann.presence = Presence::Active`.
     b. If the side doesn't match, create a fresh bracket instead.
  2. Check distance: `should_reposition(bracket.entry.price, last_price, market_cache.get(symbol).gatr_abs)`.
  3. If repositioning needed, call `reposition_bracket()`.
  4. Call `sync_panel_from_bracket()` to repopulate panel inputs from the (possibly repositioned) bracket.
  5. If no hidden bracket exists, fall through to Slice 3's creation logic.
- **`should_reposition(entry, current, gatr_abs)`**: Returns `true` when `|entry - current| > gatr_abs.unwrap_or(current * 0.05)`. Fallback to 5% when G.ATR is unavailable.
- **`reposition_bracket(bracket, current_price)`**: Compute `delta = current_price - bracket.entry.price`. Apply delta to all legs (entry, TP, SL). Preserves R:R shape.
- **Symbol change**: When the user changes symbols while a bracket is active, the bracket is hidden in the old symbol's store. If the user returns to that symbol, recall logic finds it. The existing `handle_order_panel_symbol_change` in app.rs needs to handle the `bracket_active.is_none()` + `bracket_annotation_id.is_some()` case (hidden bracket from a previous session).
- **App restart re-linking**: Order panel state (`bracket_annotation_id`) does NOT survive restart. Hidden bracket annotations persist in JSON. On next app launch, panels must re-link to their hidden brackets. The algorithm:
  1. **Where**: In `OrderPanel::from_config()` (or immediately after, during app initialization), after the annotation store has been loaded from disk.
  2. **Query**: For each panel's symbol, search `annotation_store.get(symbol)` for the first annotation matching ALL of: `kind == AnnotationKind::OrderBracket`, `presence == Presence::Hidden`, bracket `status == BracketStatus::Draft`, bracket `saved == true`.
  3. **Ownership**: First panel to initialize for a given symbol claims the hidden bracket by setting `bracket_annotation_id`. Subsequent panels for the same symbol get no re-link (they create fresh brackets on BUY/SELL).
  4. **No match**: If no hidden Draft bracket exists for the symbol, `bracket_annotation_id` stays `None` (fresh bracket created on next BUY/SELL).
**Testing**:
- `should_reposition(100.0, 100.0, Some(5.0))` → false (0 < 5).
- `should_reposition(100.0, 106.0, Some(5.0))` → true (6 > 5).
- `should_reposition(100.0, 106.0, None)` → true (6 > 5% of 100 = 5.0).
- `reposition_bracket` with entry=100, tp=102, sl=99 and current=110 → entry=110, tp=112, sl=109.
- Round-trip: click BUY → save bracket → click X → bracket hidden → click BUY → bracket reappears with correct panel inputs.
- Rapid toggle (5+ times): no orphaned annotations, no duplicates, consistent state.
- Symbol change while hidden: bracket stays in old symbol, re-appears when user returns.
- App restart re-link: create saved bracket → hide → serialize to disk → simulate restart (reload from JSON) → panel re-links to hidden bracket via re-link algorithm.
- App restart re-link with two panels for same symbol: only the first panel claims the hidden bracket.
**Done when**: Saved brackets survive [X] toggle, persist to disk, recall near current price, panel inputs are repopulated, and re-link works across app restarts.

### Slice 6: Mouse wheel on order panel price inputs
**Goal**: Scrolling the mouse wheel over TP, SL, limit, or stop price inputs increments/decrements the value.
**Depends on**: Slice 4 (so that input changes sync to chart)
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app/views.rs` — Wrap each price input row (TP, SL, limit, stop) in `mouse_area().on_scroll()`, following the pattern at `views.rs:2615-2621`
- `desktop/win/crates/midas-app/src/app.rs` — Add `OrderPanelAction::StepPrice { field: PriceField, delta: f64 }` variant
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — Add `enum PriceField { Tp, Sl, LimitPrice, StopPrice }`
**Key implementation details**:
- For **each** of the four price inputs (TP, SL, Limit, Stop), wrap the input row in `mouse_area`:
  ```rust
  let (coarse_step, _fine_step) = midas_chart::price_step_for(current_price);
  let field = PriceField::Tp; // varies per input
  let price_row = mouse_area(inner_row).on_scroll(move |delta| {
      let lines = match delta {
          ScrollDelta::Lines { y, .. } => y,
          ScrollDelta::Pixels { y, .. } => y / 50.0,
      };
      Message::OrderPanel(panel_id, OrderPanelAction::StepPrice {
          field,
          delta: coarse_step * lines as f64,
      })
  });
  ```
- `current_price` for step calculation comes from `panel.state.last_price.unwrap_or(100.0)`.
- The `StepPrice` handler: parse current string → add delta → clamp to > 0.0 → format with `{:.2}` → dispatch to the corresponding `SetTpValue` / `SetSlValue` / etc. (which triggers Slice 4's bidirectional sync).
- Consider extracting a `fn price_input_with_scroll(...)` helper to avoid duplicating the `mouse_area` wrapper four times.
**Testing**:
- `StepPrice { field: Tp, delta: 0.05 }` with current tp_value "192.00" → produces "192.05".
- `StepPrice` with negative delta decrements correctly.
- `StepPrice` with unparseable current value (empty string) → no-op or sets to 0.0 + delta.
- Wheel-adjusted values sync to chart via Slice 4 path.
**Done when**: Mouse wheel over price inputs adjusts prices and chart lines move accordingly.

### Dependency Summary

```
Slice 1 (G.ATR abs) ────────────────────────┐
                                             │
Slice 2 (Dashed lines) ─── soft ──┐         │
                                   │         │
Slice 3 (Instant bracket) ────────┤         │
        │                          │         │
        ├── Slice 4 (Sync) ───────┤         │
        │        │                 │         │
        │        └── Slice 6 (Wheel)         │
        │                                    │
        └── Slice 5 (Hide/Recall/Reposition) ┘
```

**Parallelizable**: Slices 1, 2, and 3 can all start in parallel (Slice 3's dependency on 2 is visual polish, not a technical blocker — brackets render with solid lines until Slice 2 lands).

**Critical path**: Slice 3 → Slice 4 → Slice 6.

**Recommended schedule**:
- Phase A: Slices 1, 2, 3 in parallel → test individually
- Phase B: Slices 4 and 5 in parallel (4 needs 3; 5 needs 1+3)
- Phase C: Slice 6 (needs 4)

## Risks & Unknowns

1. **Dashed line instance count.** A full-width dashed line at 6px dash + 4px gap across a 2000px viewport = ~200 `GridLineInstance` rects per line. With 3 legs = ~600 per bracket. Ultra-wide (3840px) = ~1150 per bracket. Still trivially fast for GPU (`GridLineInstance` is ~32 bytes; modern GPUs handle 100k+ instances). **Mitigation**: Only segment Draft/Pending brackets; Active brackets stay solid.

2. **Panel string ↔ f64 round-trip precision.** Formatting f64 with `{:.2}` and parsing back can drift by epsilon. **Mitigation**: Always snap to tick after parse. The `price_step_for()` returns clean decimal increments (0.01 or 0.05).

3. **G.ATR availability for repositioning.** Symbols with < 3 daily bars won't have G.ATR. **Mitigation**: Fall back to 5% of current price as the distance threshold.

4. **Multiple order panels for the same symbol.** If two panels target AAPL, which panel owns the hidden bracket? **Mitigation**: V1 supports one bracket per symbol. Second panel creates a fresh bracket. Document this limitation.

5. **App restart re-linking.** Panel state (`bracket_annotation_id`) doesn't survive restart. Hidden annotations persist in JSON. **Mitigation**: On panel initialization, scan annotation store for hidden Draft brackets matching the panel's symbol and re-link.

6. **3-click tool coexistence.** User can have both an instant bracket annotation and a 3-click tool in progress. **Mitigation**: No interaction between the two flows. If confusing, a future slice can add mutual exclusion.

## Testing Strategy

All testing follows project conventions:
- **Unit tests** in inline `#[cfg(test)]` modules or sibling `tests.rs` files (per crate convention)
- **Sans-IO tests** for chart logic (no GPU, no iced — pure state assertions)
- `cargo test --workspace` in `desktop/win/` must pass
- `cargo clippy --workspace -- -D warnings` must pass

Key test categories:
- G.ATR absolute value extraction (Slice 1)
- Dashed line segmentation geometry (Slice 2)
- Bracket creation with default prices + panel sync (Slice 3)
- Bidirectional sync round-trips: panel → annotation → panel, chart → annotation → panel (Slice 4)
- Hide/recall/reposition state machine, rapid toggle stability, symbol change edge cases (Slice 5)
- Price step calculations and scroll delta conversion (Slice 6)
- Persistence round-trip: create → hide → serialize → deserialize → recall → verify prices match (Slice 5)

## Non-Goals / Out of Scope

- **G.ATR-based default TP/SL offsets.** V1 uses fixed 1%/0.5%. Configurable or ATR-based defaults are a follow-up.
- **Multiple brackets per symbol.** V1 supports one active Draft bracket per symbol per panel.
- **Bracket persistence in DuckDB.** Stays in JSON files. DB migration is a separate feature.
- **Undo/redo for bracket edits.** Out of scope.
- **Custom tick sizes per instrument.** Uses `price_step_for()` (price-based) for V1. Per-instrument tick sizes from DuckDB `min_tick` column are a future enhancement.
- **Shift+wheel fine step.** V1 uses coarse step only. Fine step modifier is a follow-up.
- **Unifying `snap_to_tick` with `price_step_for`.** V1 accepts the inconsistency between chart drag snapping and panel input stepping.

## Review Notes

**Critique findings incorporated:**
- Fixed file paths: all references to `midas-core/src/atr/` now use the full `desktop/win/crates/midas-core/` prefix.
- Added `sync_panel_from_bracket()` helper and explicit panel input population on create/recall (was missing from draft).
- Added chart drag → panel update path to Slice 4 (was missing from draft).
- Clarified `saved` flag semantics and how they interact with [X] behavior.
- Documented symbol change behavior for hidden brackets.
- Relaxed Slice 2→3 dependency (soft, not hard) to enable better parallelization.
- Added app restart re-linking to Slice 5.
- Expanded acceptance criteria for Slices 4 and 5 with edge case tests.

**Evaluation findings incorporated:**
- Explicit removal of `draft_bracket_cache` and all `sync_draft_cache()` call sites added to Slice 3 to prevent dual-state bugs.
- App-restart re-linking algorithm fully specified in Slice 5: where it runs, the matching query (Hidden + Draft + saved), ownership semantics for multi-panel conflicts, and dedicated test cases.
- `DragBracketLeg` handler location pinpointed in Slice 4 (app.rs:3870, chart_widget.rs:1130-1134).
- `segmented_line()` placement documented as prudent-deliberate debt with promotion path to shared utility.

**Trade-offs the user should review:**
1. **1% default TP/SL** — Simple but arbitrary. Consider whether G.ATR-based defaults should be V1 instead.
2. **Saved brackets only survive [X]** — Unsaved drafts are still deleted on [X]. If the user expects ALL brackets to survive, the `saved` flag check should be removed.
3. **One bracket per symbol** — If the user wants to maintain multiple draft brackets per symbol (e.g., different entry types), this needs a more complex recall mechanism.
