# Feature: Order Bracket Chart Entry (BUY/X/SELL Toggle + Interactive Bracket Lines)

## Overview

The current order entry workflow requires either manual price entry in the order panel form or a 3-click bracket tool on the chart. Both are slow when the user just wants a market order with a stop loss. This feature reduces that to: click [BUY] or [SELL], drag the SL to the desired level, click [Submit].

Add an [X] toggle between the existing [BUY] and [SELL] toggles in the order panel, creating a 3-way [BUY][X][SELL] group. When BUY or SELL is active, a market price entry line + optional draggable stop loss line appear on the chart, anchored right with price labels and action buttons ([SL], [Save], [Submit], [X]). The [X] toggle clears uncommitted brackets but persists their last values per instrument so users don't re-enter prices when returning. The existing toggles gain new behavior: activating either one now places an interactive bracket on the chart.

## Research Summary

### Codebase Analysis

**Existing infrastructure (~70% built):**
- `OrderBracket` data model (`midas-chart/src/widget/order_bracket/mod.rs`) — entry + optional TP + optional SL, `BracketSide`, `BracketStatus` (Draft/Pending/Active/Closed/Cancelled), `LegRole` enum, label formatting, R:R calculation
- `compute_bracket()` — produces `WidgetOutput` with GPU lines (Layer 7), zone fills (Layer 6), `WidgetLabel` items (Layer 10 iced overlay), and `HitZone`s for TP/SL drag
- Interaction state machine (`interaction/mod.rs`) — `DraggingBracketLeg` mode with grab-offset, entry-price clamping, side constraints; `handle_mouse_released()` resolves click vs drag via DRAG_THRESHOLD_PX (4px)
- `AnnotationStore` (`midas-app/src/annotation_store/mod.rs`) — per-symbol storage with generation counters for dirty tracking; methods: `add()`, `remove()`, `update()`, `find()`, `get()`
- `OrderPanel` / `OrderPanelState` (`midas-app/src/order_panel/mod.rs`) — dockable panel with `side: OrderSide` (Buy/Sell), quantity, TP/SL inputs, validation, `RiskReward` calculation
- `OrderAnnotationLink` — maps annotation IDs to broker order UUIDs
- `BracketTool` (`bracket_tool/mod.rs`) — existing 3-click placement (remains as alternative)
- `ChartRenderSnapshot` passes `bracket_annotations` to the shader widget
- Message variants: `ChartCreateBracket`, `ChartDragBracketLeg`, `OrderPanelMsg`, `AddOrderPanel`
- `HitZoneKind` variants: `LevelLine`, `BracketEntry`, `BracketTP`, `BracketSL`, `BracketZone`, `MarkerIcon`, `NoteBody`, `VolumeProfileBar`

**Key integration seams:**
- `OrderPanel.state.side` already has Buy/Sell — `bracket_active` is a new separate field for the toggle mode
- `AnnotationStore` stores live `OrderBracket` annotations per symbol; draft bracket cache is separate (see Design Decision 2)
- `compute_bracket()` renders lines + labels; needs new label variants for right-anchored action buttons
- Interaction layer handles bracket leg dragging; needs click-handling for button hit zones (no drag)

### Best Practices & Idiomatic Approach

- **Right-anchored button labels via iced overlay** (Layer 10): Button positions are `WidgetLabel`s with corresponding `HitZone` overlays. Clicks on button hit zones (drag distance < 4px threshold) emit `ChartAction` variants. No changes to `WidgetLabel` struct needed.
- **Sans-IO boundary**: All bracket state mutations flow through `ChartAction` -> `Message` -> `MidasApp::update()`. The chart crate never mutates annotation data.
- **Per-instrument draft cache**: Separate `HashMap<(OrderPanelId, String), OrderBracket>` in `MidasApp` (not in AnnotationStore). Clean separation between "active visible annotations" and "cached inactive drafts."
- **Side-colored entry line**: TradingView pattern — entry line is green for long, red for short. SL always red. Reuse existing `BRACKET_TP_COLOR` / `BRACKET_SL_COLOR` constants for consistency.
- **Brightness = commitment visual model**: Alpha progression communicates lifecycle stage at a glance — dark drafts (0.55), brighter saved drafts (0.80), full brightness live orders (1.0), dimmed historical (0.30/0.20). The existing `leg_style()` alpha values are revised to create a coherent progression. Partial fills show text-based progress (`◑ 50/100sh`) on the entry label.
- **Entry price from market data events**: Update entry price only on market data change messages (not every frame), with tick-size threshold to minimize dirty-flag bumps.

## Design Decisions

### Decision 1: How to represent the BUY/X/SELL toggle state
**Context**: The existing order panel already has [BUY] and [SELL] toggles (via `OrderSide`). We're adding [X] between them for a 3-way group. Currently `OrderSide` is `Buy | Sell` with no neutral state.
**Options**:
1. Add `OrderSide::None` variant — simple, but pollutes the enum for all order submission code
2. Separate `bracket_active: Option<OrderSide>` — `None` = [X] active, `Some(Buy/Sell)` = bracket active
3. New `BracketToggle` enum: `Buy | Neutral | Sell`

**Recommendation**: Option 2 — `bracket_active: Option<OrderSide>` as a new field on `OrderPanelState`. The existing `side: OrderSide` field remains unchanged for order submission forms. The two fields are related but serve different purposes: `side` is always Buy or Sell (for the form inputs), `bracket_active` controls whether a bracket is shown on the chart. When [BUY] is pressed, both `side = Buy` and `bracket_active = Some(Buy)` are set. When [X] is pressed, `bracket_active = None` but `side` retains its last value. Confidence: **high**.

### Decision 2: Where to store uncommitted bracket state per instrument
**Context**: When user toggles [X], we need to remember the SL price they set so it's restored when they toggle back.
**Options**:
1. In `AnnotationStore` with `Presence::Hidden` — brackets stay in storage but are invisible
2. In a new `HashMap<(OrderPanelId, String), OrderBracket>` on `MidasApp` — separate from live annotations
3. In `OrderPanelState` — extend existing panel state

**Recommendation**: Option 2 — separate `draft_bracket_cache` HashMap on `MidasApp`, keyed by `(OrderPanelId, symbol)`. Reasons:
- AnnotationStore is designed as single source of truth for *visible* annotations, not a session cache. Using `Presence::Hidden` would pollute dirty tracking and risk accidental serialization.
- Keying by `(OrderPanelId, symbol)` prevents multi-panel conflicts — each panel tracks its own draft per symbol.
- When BUY/SELL toggled: create `OrderBracket` in AnnotationStore (visible Draft) AND save a copy in `draft_bracket_cache`. When [X] toggled: remove from AnnotationStore, cache remains. When BUY/SELL re-toggled: restore from cache into AnnotationStore.
- The `OrderPanelState` also tracks `bracket_annotation_id: Option<AnnotationId>` to link the panel to its live annotation.

Confidence: **high**.

### Decision 3: Action buttons on bracket lines — interaction model
**Context**: Need clickable [SL], [Save], [Submit], [X] buttons anchored to the right edge of bracket lines.
**Options**:
1. Extend `WidgetLabel` to support click actions — labels become dual-purpose
2. New `WidgetButton` struct in `WidgetOutput`
3. Use `HitZone` regions mapped to button positions — buttons are styled labels with hit zones

**Recommendation**: Option 3. Add `HitZoneKind` variants: `BracketSubmit`, `BracketSave`, `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`. Labels are positioned `WidgetLabel`s. Hit zones overlay label positions with `CursorIcon::Pointer` (not `ResizeNS`).

**Click-vs-drag handling**: In `handle_mouse_released()`, when the click lands on a button HitZone and total drag distance < `DRAG_THRESHOLD_PX` (4px), emit the corresponding `ChartAction`. If drag exceeds threshold, treat as failed drag (ignore). This reuses the existing PendingDrag → click resolution logic.

Confidence: **high**.

### Decision 4: Entry price source and update timing
**Context**: Market orders execute at current market price. Entry line needs a price.
**Recommendation**: Entry line tracks `last_price` from market cache. Updated only on market data change events in `update()` (not every frame). Skip update if `abs(new_price - old_price) < 0.01` (stock tick size) to minimize dirty-flag bumps. During SL drag, entry price is frozen at the drag-start value (captured in `DraggingBracketLeg.entry_price`) to prevent constraint violations mid-drag. Confidence: **high**.

### Decision 5: [Save] behavior
**Context**: User wants to pin a bracket so [X] toggle doesn't clear it.
**Recommendation**: Add a `saved: bool` field to `OrderBracket`. When [Save] clicked, set `saved = true`. Saved Draft brackets are NOT removed from AnnotationStore when [X] is toggled — they remain visible. Only explicitly cancelling (clicking [X] on entry line of a saved bracket) removes them. The `BracketStatus` enum is unchanged (no new variant). Confidence: **high**.

### Decision 6: Visual treatment per bracket lifecycle stage
**Context**: Users need to instantly distinguish draft sketches from live orders from historical fills. Brightness and line style must communicate commitment level at a glance.
**Options**:
1. Color-only differentiation (different hues per status)
2. Alpha/brightness progression (darker = less committed, brighter = more committed)
3. Line style + alpha combo (current approach in `leg_style()`, extended)

**Recommendation**: Option 3 — combine line style, alpha, and label format. Each stage has a distinct visual signature:

| Status | Line Style | Alpha | Label Format | Rationale |
|---|---|---|---|---|
| Draft (unsaved) | Dashed 6px/4px | 0.50 | `BUY @ 171.59` | Sketch feel, clearly uncommitted |
| Draft (saved) | Dashed 6px/4px | 0.65 | `BUY @ 171.59` | Brighter = more committed than unsaved |
| Pending | Dotted 4px | 0.80 | `BUY @ 171.59  ⏳` | Brighter than saved — money is committed, "in transit" |
| PartialFill | Solid 1.5px | 0.90 | `BUY @ 171.59  ◑ 50/100sh` | Nearly live, fill progress visible |
| Active | Solid 1.5px | 1.00 | `▲ 171.59  100sh` | Full brightness = live money |
| Closed | Solid 1.0px | 0.30 | `▲ 171.59  100sh` | Historical, dimmed |
| Cancelled | Solid 1.0px | 0.20 | `▲ 171.59  100sh` | Dead, nearly invisible |

Alpha is strictly monotonic for pre-terminal states (0.50 → 0.65 → 0.80 → 0.90 → 1.0). Each step toward live money gets brighter. The dotted line style already distinguishes Pending from Draft; alpha reinforces the escalation rather than fighting it. The `saved` distinction: `leg_style()` checks `self.saved` when `self.status == Draft` to select between 0.50 and 0.65 alpha. Confidence: **high**.

## Implementation Plan

### Slice 1: Draft Cache + Toggle Lifecycle + Annotation CRUD
**Goal**: Wire BUY/X/SELL toggle to create/cache/remove Draft bracket annotations per instrument. Full vertical slice: state change → annotation mutation → cache update.
**Depends on**: None
**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — add `saved: bool` field to `OrderBracket` (default `false`) and `filled_qty: Option<f64>` (default `None`, set by app layer on partial/full fills for label rendering), update `Default`/constructors
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — add `bracket_active: Option<OrderSide>` and `bracket_annotation_id: Option<AnnotationId>` to `OrderPanelState`, add `OrderPanelAction::SetBracketMode(Option<OrderSide>)`
- `desktop/win/crates/midas-app/src/app.rs` — add `draft_bracket_cache: HashMap<(OrderPanelId, String), OrderBracket>` to `MidasApp`. Handle `SetBracketMode`:
  - `Some(Buy/Sell)`: First, check AnnotationStore for an existing saved Draft bracket for this `(panel_id, symbol)` — if one exists, re-link to it (`bracket_annotation_id = Some(existing_id)`) and update its side if changed. Otherwise, check `draft_bracket_cache` for `(panel_id, symbol)` — if found, restore it into AnnotationStore (update side if changed). If neither exists, create new `OrderBracket { entry: last_price, stop_loss: None, take_profit: None, side, status: Draft, saved: false }` and add to AnnotationStore. Store annotation ID in `bracket_annotation_id`.
  - `None` ([X]): If `bracket_annotation_id` is `Some(id)` and bracket is NOT saved, remove from AnnotationStore, save to `draft_bracket_cache`. If saved, leave in AnnotationStore (it remains visible on chart as a pinned bracket). Set `bracket_active = None`, `bracket_annotation_id = None`.

**Key implementation details**:
- **Centralized cache sync helper**: Add `fn sync_draft_cache(&mut self, panel_id: OrderPanelId, symbol: &str)` on `MidasApp` that copies the current bracket annotation (looked up via `bracket_annotation_id`) into `draft_bracket_cache`. ALL handlers that mutate a bracket (Slices 1, 5, 6) call this after mutation. This eliminates the bug class where AnnotationStore and cache diverge. Add `debug_assert!` in dev builds that verifies cache matches annotation after each mutation.
- Edge case: market data unavailable (`last_price` is `None`) — create bracket with `entry.price = 0.0`. Entry price will update when market data arrives (Slice 4).
- Edge case: direct BUY → SELL toggle (no [X] between) — update existing annotation's side from Long→Short (or vice versa). Call `sync_draft_cache()`. If SL exists, flip constraint side (Long SL must be below entry; Short SL must be above entry; if violated, remove SL).
- `bracket_annotation_id` unambiguously links each panel to its bracket annotation, preventing multi-panel conflicts on same symbol.

**Testing**:
- Unit: `SetBracketMode(Some(Buy))` creates annotation, stores ID in panel state
- Unit: `SetBracketMode(None)` removes annotation (if not saved), caches OrderBracket
- Unit: `SetBracketMode(Some(Buy))` after [X] restores from cache
- Unit: direct BUY→SELL flips side, handles SL constraint
- Unit: `saved = true` bracket not removed by [X]
- Unit: toggle BUY → Save → toggle [X] → toggle BUY → only ONE bracket exists in AnnotationStore (re-links to existing saved bracket, does not duplicate)

**Done when**: BUY/X/SELL toggle creates, caches, removes, and restores Draft bracket annotations. Panel tracks annotation ID. Cache keyed by (panel_id, symbol). Saved brackets are re-linked on re-toggle, never duplicated.

---

### Slice 2: Status-Aware Visual Treatment + Side-Colored Entry Line
**Goal**: Bracket lines visually communicate lifecycle stage through brightness, line style, and label format. Entry line is green for long, red for short. Each status has a distinct visual signature per Decision 6.
**Depends on**: Slice 1
**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs`:
  - Add `BRACKET_LONG_ENTRY_COLOR: [f32; 4] = [0.20, 0.78, 0.35, 1.0]` (reuse green) and `BRACKET_SHORT_ENTRY_COLOR: [f32; 4] = [0.90, 0.25, 0.25, 1.0]` (reuse red)
  - Modify `leg_style()` to implement the full visual treatment table (Decision 6):
    - For `LegRole::Entry`: use `self.side` to select green (Long) or red (Short) instead of `BRACKET_ENTRY_COLOR`
    - For `BracketStatus::Draft`: check `self.saved` — unsaved gets alpha 0.50, saved gets alpha 0.65. Both use dashed line style.
    - Other statuses keep existing style/width but use updated alpha values: Pending 0.80, PartialFill 0.90, Active 1.0, Closed 0.30, Cancelled 0.20
  - Modify `format_entry_label()` to be status-aware:
    - `Draft`: `"BUY @ {price:.2}"` / `"SELL @ {price:.2}"`
    - `Pending`: `"BUY @ {price:.2}  ⏳"` (awaiting fill)
    - `PartialFill`: `"BUY @ {price:.2}  ◑ {filled}/{total}sh"` (e.g., `"BUY @ 171.59  ◑ 50/100sh"`) — uses `filled_qty` and `quantity` fields
    - `Active`: `"▲ {price:.2}  {qty}sh"` (existing format, entry filled)
    - `Closed` / `Cancelled`: keep existing dimmed format
  - Modify `format_sl_label()`: for PartialFill/Active statuses, append projected P&L as today. For Draft, show just `"SL @ {price:.2}"`.

**Key implementation details**:
- The `saved` alpha distinction (0.50 vs 0.65) is the visual cue that a bracket has been pinned. No other visual change needed — same dashed line, same color, just brighter.
- `filled_qty` is `Option<f64>` on `OrderBracket`, set to `None` for Draft/Pending, and populated by the app layer from execution reports when fills arrive. `format_entry_label()` uses `filled_qty.unwrap_or(0.0)` for the partial fill display.
- The alpha progression is strictly monotonic for pre-terminal states (0.50 → 0.65 → 0.80 → 0.90 → 1.0) creating an intuitive "brightness = commitment" scale. Each step toward live money gets brighter. Users can glance at a chart with multiple brackets and instantly distinguish drafts from live orders from historical fills.

**Testing**:
- Update existing tests in `order_bracket/tests.rs` for new label format and alpha values
- New test: `leg_style(LegRole::Entry)` returns green color for Long, red for Short
- New test: `leg_style()` for Draft unsaved returns alpha 0.50, Draft saved returns 0.65
- New test: `leg_style()` for Pending returns alpha 0.80, Active returns 1.0
- New test: `format_entry_label()` returns "BUY @ 171.59" for Draft Long
- New test: `format_entry_label()` returns "▲ 171.59  100sh" for Active Long
- New test: `format_entry_label()` returns "BUY @ 171.59  ◑ 50/100sh" for PartialFill with filled_qty=50, quantity=100

**Done when**: Each bracket status has a distinct visual signature (line style + alpha + label format). Saved drafts are visibly brighter than unsaved. PartialFill shows fill progress text. The brightness progression from Draft → Active is intuitive.

---

### Slice 3: Right-Anchored Action Buttons (Hit Zones + Labels)
**Goal**: Render [SL], [Save], [Submit], [X] buttons right-aligned on the entry line. Render [X] on the SL line. Clickable via hit zones with correct click-vs-drag resolution.
**Depends on**: Slice 2
**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/hit_test.rs` — add `HitZoneKind` variants: `BracketSubmit`, `BracketSave`, `BracketToggleSL`, `BracketCancel`, `BracketCancelSL`. All use `CursorIcon::Pointer`.
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — extend `compute_bracket()` to emit button labels and hit zones for Draft brackets:
  - Entry line buttons (right to left, 8px spacing, from `vp_width - 8.0`):
    ```
    ──[BUY @ 171.59]────────[SL][Save][Submit][X]──|
    ```
  - Each button: `WidgetLabel` at computed x-offset + `HitZone` with matching rect
  - Button width: character count × 7.0 + 12.0 padding (for 11px font)
  - SL line button (when SL exists):
    ```
    ────────────────────────────[SL @ 169.95][X]──|
    ```
  - Only emit buttons for `BracketStatus::Draft` brackets
  - [SL] button: shown on entry line only when `bracket.stop_loss.is_none()`. When SL exists, shown as price label on SL line with [X] to remove.
  - Button colors: [Submit] `bg_color` matches side color (green/red), [X] gray `[0.4, 0.4, 0.4, 0.85]`, [Save] blue-gray `[0.35, 0.45, 0.65, 0.85]`, [SL] orange `[0.85, 0.55, 0.20, 0.85]`
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — add `ChartAction` variants:
  - `SubmitBracket { annotation_id: AnnotationId }`
  - `SaveBracket { annotation_id: AnnotationId }`
  - `ToggleBracketSL { annotation_id: AnnotationId }`
  - `CancelBracket { annotation_id: AnnotationId }`
  - `CancelBracketSL { annotation_id: AnnotationId }`
  - In `handle_mouse_released()`: when resolving a PendingDrag that didn't exceed threshold (click, at ~line 910), check button HitZones BEFORE the existing `hit_test_levels()` call. If click falls in a button HitZone, emit corresponding ChartAction and return. Otherwise fall through to existing level click handling.
  - **Hit-test priority order** (important — two systems coexist): On drag (exceeds threshold), `hit_test_bracket_legs()` runs first using direct annotation price inspection (~line 362). On click (under threshold), button HitZone rects are checked first, then `hit_test_levels()`. The existing `hit_test_bracket_legs()` does NOT use `HitZone` rects from `WidgetOutput` — it does its own price-space calculation against annotation data. Both systems coexist; buttons are click-only, leg drags are drag-only, so they don't conflict. Future: migrate `hit_test_bracket_legs()` to use precomputed HitZone rects for consistency.
- `desktop/win/crates/midas-app/src/app.rs` — add Message variants:
  - `ChartBracketSubmit(ChartId, AnnotationId)`
  - `ChartBracketSave(ChartId, AnnotationId)`
  - `ChartBracketToggleSL(ChartId, AnnotationId)`
  - `ChartBracketCancel(ChartId, AnnotationId)`
  - `ChartBracketCancelSL(ChartId, AnnotationId)`
  - (Handlers implemented in Slice 5)
- `desktop/win/crates/midas-app/src/chart_widget.rs` — map new `ChartAction` variants to corresponding `Message` variants in the existing action-to-message translation logic.

**Key implementation details**:
- Button hit zone rects are computed from label position and estimated text width. Each rect is `[x_start, y - 8.0, x_start + width, y + 8.0]` (16px touch target height).
- **Narrow viewport overflow**: Button positions are computed right-to-left. If a button's `x_start` falls below 0.0, that button and all further buttons are skipped (not emitted). Priority order for dropping: [SL] drops first, then [Save], then [X]. [Submit] and the price label are always emitted if they fit. This ensures the most critical actions remain accessible on narrow charts.
- Buttons are only emitted for `BracketStatus::Draft`. Pending/Active brackets show labels without buttons.
- When no market data available (entry price = 0.0), [Submit] button is NOT emitted (prevents submitting invalid order).

**Testing**:
- Unit: `compute_bracket()` for Draft Long bracket emits correct label count and hit zone count
- Unit: button hit zone rects are within viewport, no overlaps
- Unit: button positions shift correctly when SL is present vs absent
- Unit: no buttons emitted for Active/Pending brackets
- Unit: no [Submit] button when entry price is 0.0

**Done when**: Draft brackets render with right-anchored buttons. Click on button hit zone emits correct ChartAction → Message.

---

### Slice 4: Toggle UI + Entry Price Tracking
**Goal**: Order panel shows [BUY][X][SELL] toggle group. Entry line price tracks market data.
**Depends on**: Slice 1
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app/views.rs` — modify `view_order_body()`:
  - Insert [X] button between the existing [BUY] and [SELL] buttons, creating a 3-way toggle group
  - Existing [BUY]/[SELL] styling already handles active/inactive states (green/red active, dim inactive)
  - [X] button: neutral dark gray when active (`bracket_active == None`), dim when inactive
  - Wire existing [BUY]/[SELL] `on_press` handlers to also call `SetBracketMode(Some(Buy/Sell))`
  - [X] button emits `Message::OrderPanelMsg(id, OrderPanelAction::SetBracketMode(None))`
  - When `bracket_active.is_some()` and `last_price.is_none()`, show "Waiting for market data..." below toggle
- `desktop/win/crates/midas-app/src/app.rs` — entry price tracking:
  - **Hook point**: `Message::MarketSnapshotLoaded(symbol, Ok(buffer))` handler at ~line 2860. After `self.market_cache.insert(symbol, snapshot)` (line 2867), add bracket entry price sync: iterate all `order_panels`, find any with `bracket_active.is_some()` and matching symbol, update the Draft bracket's entry price via `annotation_store.update()` if `abs(new_price - old_price) >= 0.01`.
  - Also sync on `Message::DataLoaded` for chart candle loads (covers the initial-load path where watchlist snapshot hasn't arrived yet).
  - Batch all bracket updates for the same symbol into one annotation store call to avoid multiple dirty-flag increments.

**Key implementation details**:
- The [BUY] and [SELL] buttons in the toggle group handle the case where user clicks BUY while SELL is already active (direct side flip without going through [X]).
- Market data update path: `Message::MarketSnapshotLoaded` at app.rs ~line 2860 is the primary hook. `self.market_cache.insert(symbol, snapshot)` populates the cache; bracket entry price sync runs immediately after, before returning `Task::none()`.

**Testing**:
- Visual: toggle BUY → entry line appears at market price. Toggle X → line disappears. Toggle BUY → line reappears.
- Unit: entry price updates when market data changes by >= $0.01
- Unit: entry price does NOT update when delta < $0.01

**Done when**: Order panel shows 3-way toggle with correct styling. Entry line tracks live market price with throttling.

---

### Slice 5: Button Handlers (SL Toggle, Save, Submit, Cancel)
**Goal**: All bracket action buttons are functional. SL can be added, dragged, and removed. Brackets can be saved, submitted, and cancelled.
**Depends on**: Slice 3, Slice 4
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — add `validate_bracket(bracket: &OrderBracket, quantity: f64) -> Vec<(String, String)>` that validates bracket f64 data directly (entry > 0, SL side constraints, quantity > 0). Separate from form-based `validate_panel()`.
- `desktop/win/crates/midas-app/src/app.rs` — implement handlers for new Message variants:
  - `ChartBracketToggleSL`: If SL exists, remove it (`stop_loss = None`). If not, add SL at 2% below entry (Long) or 2% above entry (Short). If entry price is 0.0, skip (no-op until market data available).
  - `ChartBracketCancelSL`: Remove SL from bracket (`stop_loss = None`). Update `draft_bracket_cache`.
  - `ChartBracketSave`: Set `bracket.saved = true` via `annotation_store.update()`. Update `draft_bracket_cache`.
  - `ChartBracketSubmit`: Validate the bracket annotation directly (NOT via `validate_panel()` — that function validates form string inputs, not bracket f64 data). Bracket-specific validation checks: (1) `entry.price > 0.0` (market data available), (2) if SL exists, SL respects side constraints (Long: SL < entry, Short: SL > entry), (3) quantity > 0 — resolve from `bracket.quantity` if `Some`, else parse from `panel.state.quantity` via panel_id lookup, else return validation error "Quantity required". If validation fails, set `panel.state.errors` with descriptive messages (e.g., "No market price available", "Stop loss must be below entry for BUY") and return. If valid, create broker order via existing `create_market_bracket` flow (from `OrderPanelAction::ConfirmYes` handler pattern). Transition bracket to `BracketStatus::Pending`. Create `OrderAnnotationLink`. If broker call fails, log error, keep bracket as Draft, set error message "Broker connection failed — retry with [Submit]".
- Existing `Message::BracketStatusChanged` handler (app.rs ~line 3546) already updates `BracketStatus` on chart annotations from broker lifecycle events. Extend it to also set `filled_qty` from execution reports when status transitions to `PartialFill` or `Active`. The fill data flows: broker execution report → `BracketStatusChanged` message → `annotation_store.update()` sets `filled_qty` → next frame `compute_bracket()` picks it up → label renders fill progress.
- **Fill quantity sourcing**: The existing `BrokerEvent::BracketStatusChanged` includes `entry_fill_price: Option<f64>` but not fill quantity. Two options: (a) extend `BrokerEvent::BracketStatusChanged` to include `filled_qty: Option<f64>`, derived by the broker layer from summing execution reports on the parent order, or (b) use the existing `OrderAnnotationLink.parent_order_id` to correlate `BrokerEvent::OrderStatusChanged` events (which carry `filled_qty`) with bracket annotations. Option (a) is cleaner — keeps the app layer from needing to cross-reference order IDs. The broker layer already tracks fills per order; surfacing the aggregate in the bracket event is a one-line addition.
  - `ChartBracketCancel`: If bracket is saved, confirm before removing. Remove from AnnotationStore. Update `draft_bracket_cache` (remove entry). Set `bracket_active = None`, `bracket_annotation_id = None`.

**Key implementation details**:
- SL toggle: the newly added SL line is immediately draggable via existing `DraggingBracketLeg` mode. No new drag code needed.
- [Submit] follows the same broker integration path as the existing `OrderPanelAction::ConfirmYes` handler (app.rs ~line 2475). Reuse `create_bracket_annotation()` and broker bridge patterns.
- [Submit] disabled when: no market data, entry price = 0.0, validation errors. Enforced by not emitting the button HitZone (Slice 3) and by validating in the handler.
- Broker disconnect: if `create_market_bracket()` returns Err, bracket stays Draft. Error shown in `state.errors`. User can retry.

**Testing**:
- Unit: [SL] toggle adds SL at 2% offset, removes SL when toggled again
- Unit: [Save] sets `saved = true`
- Unit: [Submit] with valid data transitions to Pending
- Unit: [Submit] with entry.price = 0.0 returns "No market price available" error
- Unit: [Submit] with SL on wrong side returns constraint error
- Unit: [Submit] with validation errors does not call broker
- Unit: [Cancel] removes annotation and clears panel state
- Unit: [Cancel] on saved bracket removes it
- Integration: full flow — toggle BUY, add SL, drag SL, save, submit → bracket transitions to Pending

**Done when**: All action buttons work. SL add/remove/drag functional. Save pins bracket. Submit creates broker order. Cancel removes bracket.

---

### Slice 6: Per-Instrument Persistence + Edge Cases
**Goal**: Bracket state survives instrument switching and timeframe changes. Polish edge cases.
**Depends on**: Slice 5
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — symbol change handling:
  - When an OrderPanel's symbol changes (via link group propagation or user input), if `bracket_active.is_some()`: remove current Draft bracket from AnnotationStore (unless saved), save to `draft_bracket_cache` under old symbol key via `sync_draft_cache()`. Look up `draft_bracket_cache` for `(panel_id, new_symbol)`. If found, restore into AnnotationStore. If not, keep `bracket_active` but create fresh bracket for new symbol.
  - **Pending/Active brackets on symbol change**: Pending and Active brackets are annotation-store-owned and symbol-scoped. They remain visible on any chart showing that symbol, regardless of panel state. Symbol change on the order panel does NOT remove or hide them — they represent live broker orders and must stay visible until filled, closed, or cancelled. The panel's `bracket_annotation_id` is cleared (it now tracks the new symbol's Draft bracket), but the `OrderAnnotationLink` continues to map the old bracket to its broker order.
  - When chart data loads for a new symbol and a panel has `bracket_active.is_some()` with `entry.price == 0.0`, update entry price from newly loaded data.
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — `to_config()`/`from_config()` support: persist `bracket_active` toggle state (but NOT the draft cache — drafts are session-only). Default `bracket_active = None` if missing from config (backward compatible with older config files — no migration needed).
- `desktop/win/crates/midas-app/src/app/persistence.rs` — include `bracket_active` in config save/restore so toggle state survives app restart.

**Key implementation details**:
- Timeframe change: Draft brackets show on all timeframes (no `visible_timeframes` constraint). Bracket `entry.price` is a price, not a candle index, so it renders correctly regardless of timeframe.
- Collapsed gap mode: `compute_bracket()` uses `camera.price_to_y()` which works correctly in both gap modes (price → Y is gap-mode agnostic). Hit zones are recomputed each frame.
- Rapid toggle debounce: Not needed — iced's Elm architecture processes messages sequentially. Each toggle is atomic; no race conditions.
- Window resize: buttons and hit zones are recomputed each frame from `vp_width`, so they naturally adapt.
- Multiple panels on same symbol: keyed by `(panel_id, symbol)`, so each panel has independent drafts.

**Testing**:
- Integration: switch symbol → bracket cached for old symbol → switch back → bracket restored
- Edge: toggle BUY with no market data → bracket created → data loads → entry price updates
- Edge: BUY→SELL direct toggle → side flips, SL constraint adjusted
- Edge: resize window during bracket display → buttons reposition correctly
- Edge: collapsed gap mode → bracket lines render at correct Y position

**Done when**: Bracket state persists across instrument switches. All edge cases handled.

### Dependency Summary

```
Slice 1 (toggle + lifecycle + cache) ──┬──> Slice 2 (side-colored lines)
                                        │         │
                                        │         v
                                        │    Slice 3 (action buttons + hit zones)
                                        │         │
                                        ├──> Slice 4 (toggle UI + price tracking)
                                        │         │
                                        │         v
                                        │    Slice 5 (button handlers)
                                        │         │
                                        │         v
                                        └──> Slice 6 (persistence + edge cases)
```

- **Slices 2 and 4 can be parallelized** (both depend only on Slice 1)
- Slice 3 depends on Slice 2 (label format changes)
- Slice 5 depends on Slices 3 and 4
- Slice 6 is the integration/polish slice
- **Critical path**: 1 → 2 → 3 → 5 → 6

## Risks & Unknowns

1. **Entry price update frequency**: If market data updates every tick, annotation dirty-flag bumps could cause unnecessary GPU re-uploads. **Mitigation**: Only update entry price if `abs(new - old) >= 0.01` (stock tick size). During SL drag, entry price is frozen at drag-start value (already captured in `DraggingBracketLeg.entry_price`).

2. **Button label width estimation**: `WidgetLabel` doesn't store measured width. Hit zones need accurate width. **Mitigation**: Use fixed-width estimates (char_count × 7.0 + 12.0 padding at 11px font). Sufficient for fixed button texts ([SL], [Save], [Submit], [X]). Can refine with text measurement later if needed.

3. **SL default offset**: 2% of entry price as default. Acceptable for stocks; may need adjustment for futures/forex in future. Not configurable in V1.

4. **Broker disconnect on Submit**: If `create_market_bracket()` fails, bracket stays Draft, error shown in panel. User can retry. No timeout logic in V1 — broker bridge is synchronous (blocks briefly). Future: add timeout + retry.

5. **3-click BracketTool coexistence**: Both the toggle-driven bracket and the 3-click tool can create Draft brackets for the same symbol. Multiple Draft brackets per symbol are valid (each identified by `AnnotationId`). The panel's `bracket_annotation_id` only tracks the toggle-driven one.

6. **Dual state synchronization (AnnotationStore + draft_bracket_cache)**: Every handler that mutates a bracket must call `sync_draft_cache()` afterward. Missing a sync call means the user loses their SL price on re-toggle. **Mitigation**: Centralized `sync_draft_cache()` helper called from all mutation paths. `debug_assert!` in dev builds verifies cache consistency after each `update()` cycle.

7. **Two hit-testing systems**: Button clicks use precomputed `HitZone` rects from `compute_bracket()`. Bracket leg drags use direct annotation price inspection in `hit_test_bracket_legs()`. These coexist without conflict (buttons are click-only, legs are drag-only), but the dual codepath is technical debt. **Mitigation**: Document priority order in interaction layer. Migrating leg hit-testing to use HitZone rects is a follow-up.

## Testing Strategy

- **Sans-IO unit tests** (midas-chart): New `HitZoneKind` variants, button position calculations, label formatting (status-conditional), `compute_bracket()` output for Draft brackets with/without SL, side-colored entry line.
- **AnnotationStore + cache unit tests** (midas-app): Draft lifecycle (create → cache → restore), `saved` flag behavior, multi-panel independence.
- **Integration tests**: Full message round-trip: toggle BUY → verify annotation → add SL → drag SL → save → submit → verify Pending. Toggle X → verify removed (unsaved) or retained (saved).
- **Edge case tests**: No market data, symbol switch, direct side flip, entry price = 0.0.
- No GPU/visual tests — all logic testable via sans-IO pattern.

## Non-Goals / Out of Scope

- **Take Profit line**: Only entry + SL for V1. TP follows the same button pattern and can be added as a follow-up.
- **Limit order entry**: Only market orders (entry at current price). Limit orders need a draggable entry line.
- **Bracket template presets**: No saved configurations. Manual SL per instrument, cached per session.
- **Multi-leg brackets**: No OCO, bracket groups, or complex order types.
- **Persist Draft brackets to disk**: Drafts are session-only. Toggle state persisted; draft cache is not.
- **3-click bracket tool deprecation**: Existing BracketTool remains as alternative placement method.
- **Undo stack for bracket edits**: No undo beyond the cache restore mechanism.

## Review Notes

Key trade-offs surfaced during review:

1. **Separate draft cache vs AnnotationStore with Presence::Hidden**: Chose separate `HashMap` cache. The AnnotationStore is designed as single source of truth for *visible* annotations with dirty-tracking side effects. Using `Presence::Hidden` for session caching would pollute generation counters and risk serialization of transient state. The cache approach is cleaner but introduces a second data path for bracket state. **Mitigation**: centralized `sync_draft_cache()` helper called from all mutation paths to prevent divergence.

2. **Fixed button width estimation vs text measurement**: Chose fixed estimation for V1. Accurate text measurement requires access to the font metrics pipeline (ab_glyph), which runs inside the render layer. Passing measured widths back to the sans-IO chart layer would violate the architecture boundary. The fixed estimate is sufficient for the small, fixed-text buttons in this feature.

3. **Entry price frozen during SL drag**: The existing `DraggingBracketLeg` mode captures `entry_price` at drag start. This means if market price moves significantly during a drag, the constraint clamping uses the stale entry price. This is acceptable — the drag lasts seconds at most, and market orders fill at whatever the current price is anyway.

4. **`saved` bool flag vs new BracketStatus variant**: Chose a separate `saved` field on `OrderBracket` rather than a `SavedDraft` status. The `BracketStatus` enum maps to broker lifecycle states (Draft → Pending → Active → Closed). "Saved" is a UI concept (pin the bracket so [X] doesn't clear it), orthogonal to broker status. A separate field avoids semantic overloading.

5. **Bracket-specific validation vs reusing `validate_panel()`**: Chose a new `validate_bracket()` function. The existing `validate_panel()` validates form string inputs (`tp_value.parse()`, `sl_value.parse()`), but the chart-driven bracket flow has prices as f64 in the `OrderBracket` struct. Calling `validate_panel()` would validate empty/unset form fields rather than the actual bracket data. The bracket-specific function validates directly: `entry.price > 0`, SL side constraints, `quantity > 0`.

6. **Two coexisting hit-test systems**: Bracket leg drags use `hit_test_bracket_legs()` which directly inspects annotation data via `camera.price_to_y()`. New button clicks use precomputed `HitZone` rects from `compute_bracket()`. Both systems are correct for their purpose (drag vs click), but maintaining two codepaths is technical debt. Chose to leave both for V1 and migrate leg hit-testing to HitZone rects as follow-up.

7. **Brightness = commitment visual model**: The alpha progression is strictly monotonic for pre-terminal states (0.50 → 0.65 → 0.80 → 0.90 → 1.0). Each step toward live money gets brighter — no exceptions. The dotted line style (Pending) and solid line (Active) provide additional visual distinction beyond alpha alone. Active (1.0) is the visual anchor. Text-based partial fill indicators (`◑ 50/100sh`) were chosen over custom widget rendering (progress bars, half-filled icons) for V1 simplicity — they use the existing `WidgetLabel` pipeline with zero new GPU primitives. Verify that the `◑` glyph (U+25D1) renders correctly in the app's font stack; fall back to `*` if missing.
