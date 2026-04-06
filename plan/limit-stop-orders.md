# Feature: Limit, Stop, and Stop-Limit Order Entry

## Overview

Extend the order bracket system to support Limit, Stop, and Stop-Limit entry orders alongside the existing Market orders. Users will select the entry type via tabs in the order panel, specify entry prices, and see draggable entry lines on the chart with type-specific labels ("LMT BUY @ 182.00"). The broker layer already supports all order types — this feature wires them through the chart and app layers.

## Research Summary

### Codebase Analysis

**Broker layer (already complete):**
- `OrderKind` enum (`crates/midas-broker/src/orders/types/mod.rs:49-55`) already has `Market`, `Limit`, `Stop`, `StopLimit`, `TrailingStop` variants
- `LocalOrder` struct has `limit_price: Option<f64>`, `stop_price: Option<f64>` fields
- `TakeProfitParams` and `StopLossParams` already support Limit/Stop/StopLimit for exit legs
- `MarketBracketParams` (`crates/midas-broker/src/orders/bracket/mod.rs:19-57`) is the only gap — hardcoded to Market entry

**Chart layer (needs extension):**
- `OrderBracket` (`desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs:14-36`) has no `entry_type` field — assumes Market
- Entry line is rendered at `bracket.entry.price` but NOT draggable — `BracketEntry` hit zone exists but interaction layer skips it for dragging
- `compute_bracket()` emits entry labels via `format_entry_label()` which is already status-aware but not type-aware
- `leg_style()` returns side-colored entry lines (green/red) — no type-based visual changes needed (labels handle type)

**App layer (needs wiring):**
- `ChartBracketSubmit` handler (`app.rs`) validates and transitions to Pending but does NOT send orders to broker — this is the critical gap
- `handle_set_bracket_mode()` creates brackets at `last_price` — needs to accept user-specified price for non-Market entries
- Order panel (`order_panel/mod.rs`) has no order type selector — only Market is implied
- `SetBracketMode` action needs to carry the entry type and optional price

**Interaction layer:**
- `DragBracketLeg` mode (`interaction/mod.rs`) already handles TP/SL dragging with side-constraint clamping
- `LegRole::Entry` exists but dragging is blocked — entry legs are skipped in `hit_test_bracket_legs()` and the `ChartDragBracketLeg` handler has an early return guard for entry legs
- Entry drag constraints are different from TP/SL: Limit buy should be at/below market, Stop buy at/above market (directional warnings, not hard blocks)

### Best Practices & Idiomatic Approach

- **Chart-owned EntryType enum**: Define `EntryType` in `midas-chart` (not use broker's `OrderKind`) to maintain sans-IO boundary. App layer maps `EntryType → OrderKind` at the bridge, matching the existing `LegRole → BracketRole` pattern.
- **Status owns line style, type via labels**: The existing visual system encodes status via line style (dashed=Draft, dotted=Pending, solid=Active) and alpha (0.50→1.0). Order type is communicated via label prefix ("LMT", "STP", "STP LMT"). This avoids conflict between two orthogonal information axes.
- **Incremental generalization**: Rather than redesigning the bracket system, extend it minimally — add `entry_type` field with `Default::Market`, add optional price fields, generalize validation.
- **Directional drag warnings**: Best practice from TradingView and thinkorswim UX — warn (visual highlight) when an entry price is on the "wrong" side of market, but never hard-block the user.

## Design Decisions

### Decision 1: Where to define the entry type enum
**Context**: The chart crate must not depend on broker types (architecture rule #1).
**Options**:
1. Use broker's `OrderKind` directly — violates sans-IO boundary
2. New `EntryType` enum in `midas-chart` — clean, mirrors broker enum
3. String-based entry type — no type safety

**Recommendation**: Option 2. Define `EntryType { Market, Limit, Stop, StopLimit }` in `midas-chart::widget::order_bracket`. The app layer maps it to `OrderKind` at the bridge boundary. This follows the existing `LegRole` (chart) → `BracketRole` (broker) pattern. No mirror type needed in desktop midas-core — the conversion happens directly in the app layer bridge code. Confidence: **high**.

### Decision 2: How to handle Stop-Limit's dual prices
**Context**: Stop-Limit orders have both a stop trigger price and a limit execution price. The existing `BracketLeg` has a single `price` field.
**Options**:
1. Add `stop_price: Option<f64>` to `OrderBracket` — simple, flat
2. Add `limit_price: Option<f64>` to `BracketLeg` — co-locate with the leg
3. Encode both in `BracketLeg.price` + a new `OrderBracket.entry_limit_price` field

**Recommendation**: Option 1 — add `entry_stop_price: Option<f64>` to `OrderBracket`. For Stop-Limit entries, `entry.price` holds the limit price and `entry_stop_price` holds the stop trigger price. For Stop entries, `entry.price` holds the stop price and `entry_stop_price` is `None`. For Limit entries, `entry.price` holds the limit price and `entry_stop_price` is `None`.

**Semantic note**: `entry.price` is the price the entry line renders at. For Market/Limit, this is the target fill price. For Stop, this is the trigger price (actual fill is approximate — depends on market conditions at trigger time). For StopLimit, this is the limit price; the stop trigger (`entry_stop_price`) is display-only in the label. Add a doc comment on `OrderBracket` clarifying this per-type convention. Note: existing functions `risk_reward()`, `dollar_risk()`, and `dollar_reward()` use `entry.price` as the fill price — this is exact for Market/Limit and approximate for Stop, which is acceptable for V1.

This keeps `BracketLeg` unchanged and avoids modifying the rendering pipeline. Confidence: **high**.

### Decision 3: How to communicate entry type visually
**Context**: Line style already encodes bracket status. Need to distinguish Market/Limit/Stop/StopLimit without a second visual channel.
**Options**:
1. Different line styles per type (conflicts with status encoding)
2. Different colors per type (conflicts with side encoding green/red)
3. Label prefix only ("LMT", "STP", "STP LMT")
4. Label prefix + small type indicator icon

**Recommendation**: Option 3. Prefix the entry label: Market has no prefix (current behavior), Limit shows "LMT", Stop shows "STP", StopLimit shows "STP LMT". The label already carries the side ("BUY"/"SELL") and price. Examples:
- Market Draft: `"BUY @ 182.00"`
- Limit Draft: `"LMT BUY @ 180.00"`
- Stop Draft: `"STP BUY @ 185.00"`
- StopLimit Draft: `"STP LMT BUY @ 185.00/184.50"` (stop/limit)

Confidence: **high**.

### Decision 4: Entry line draggability
**Context**: Market orders track last_price (non-draggable). Limit/Stop/StopLimit need user-specified prices (draggable).
**Options**:
1. Make entry always draggable, market just resets to last_price
2. Make entry draggable only when `entry_type != Market`
3. Separate draggable price line from non-draggable market line

**Recommendation**: Option 2. The entry line is draggable when `entry_type != Market` AND `status == Draft`. This reuses the existing `DragBracketLeg` interaction with `LegRole::Entry`. Market entries continue to track `last_price` automatically. The hit zone for entry is already emitted (`HitZoneKind::BracketEntry`) but currently skipped in `hit_test_bracket_legs()` — remove the skip condition for non-Market types and remove the early-return guard in the `ChartDragBracketLeg` handler. Confidence: **high**.

### Decision 5: Broker submission generalization
**Context**: `MarketBracketParams` is hardcoded for Market entry. Need to support all 4 types.
**Options**:
1. Rename/extend `MarketBracketParams` to `BracketParams` with `entry_kind` field
2. Create separate param structs per entry type (`LimitBracketParams`, etc.)
3. Use a generic builder pattern

**Recommendation**: Option 1. Add `entry_kind: OrderKind` and `entry_price: Option<f64>` to the existing struct (renamed to `BracketParams`). The broker engine's `build_market_bracket()` becomes `build_bracket()` and sets `order_type`, `limit_price`, `stop_price` on the parent `LocalOrder` based on `entry_kind`. The command enum variant `CreateMarketBracket` is renamed to `CreateBracket`. All call sites across both workspaces are updated atomically in the same commit — no backward-compat aliases needed (single-developer codebase with no downstream consumers). Confidence: **high**.

## Implementation Plan

### Slice 1: EntryType Data Model + Label Updates
**Goal**: Add `EntryType` enum and `entry_stop_price` field to `OrderBracket`. Update entry labels to show type prefix. No behavior changes — all existing brackets remain Market by default.
**Depends on**: None
**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — add `EntryType` enum (Market/Limit/Stop/StopLimit, default Market), add `entry_type: EntryType`, `entry_stop_price: Option<f64>`, and `wrong_side_warning: bool` fields to `OrderBracket` with `#[serde(default)]`, add doc comment on `entry.price` explaining per-type semantics (Market/Limit = target fill, Stop = trigger, StopLimit = limit price), update `format_entry_label()` to prepend type prefix and render warning amber when `wrong_side_warning` is true. Add `// TODO: consider EntryPrice enum to make per-type semantics compiler-enforced` comment on `OrderBracket`. Confirm that `risk_reward()`, `dollar_risk()`, and `dollar_reward()` use `entry.price` as-is — this is exact for Market/Limit and approximate for Stop (acceptable for V1).
- `desktop/win/crates/midas-chart/src/widget/order_bracket/tests.rs` — add tests for type-prefixed labels
- Update all `OrderBracket` constructors across the codebase (app.rs, order_panel/mod.rs, interaction/tests.rs) to include `entry_type: EntryType::Market, entry_stop_price: None, wrong_side_warning: false`

**Key implementation details**:
- `format_entry_label()` changes:
  - Market (no change): `"BUY @ 182.00"`
  - Limit: `"LMT BUY @ 180.00"`
  - Stop: `"STP BUY @ 185.00"`
  - StopLimit: `"STP LMT BUY @ 185.00/184.50"` where 185.00 is stop price (`entry_stop_price`) and 184.50 is limit price (`entry.price`)
- For StopLimit, the entry line renders at `entry.price` (limit price) since that's the execution price. The stop price is informational in the label.
- `EntryType` derives `Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize`

**Testing**:
- Unit: `format_entry_label()` for each entry type + side combination (8 tests: 4 types × 2 sides)
- Unit: `format_entry_label()` for StopLimit with both prices — exact match: `"STP LMT BUY @ 185.00/184.50"`
- Unit: default `EntryType::Market` — backward compat
- Unit: serde round-trip for `OrderBracket` with missing `entry_type` field (deserializes as Market)

**Done when**: Labels correctly show type prefixes for all 4 types. All existing tests pass unchanged (Market is default). Serde backward compat verified.

---

### Slice 2: Order Panel Type Selector + Limit Order UI
**Goal**: Add order type tabs to the order panel. When Limit is selected, show a price input and create brackets at the user-specified price (not market price).
**Depends on**: Slice 1
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — add `entry_type: EntryType` field to `OrderPanelState` (default Market), add `limit_price: String` field for text input, add `OrderPanelAction::SetEntryType(EntryType)` and `SetLimitPrice(String)` variants
- `desktop/win/crates/midas-app/src/app/views.rs` — add 4-tab order type selector row (Market/Limit/Stop/Stop Limit) in `view_order_body()` above the side toggle. When Limit selected, show price input field. Styling: active tab has accent color, inactive tabs dim.
- `desktop/win/crates/midas-app/src/app.rs` — handle `SetEntryType` and `SetLimitPrice` actions. Modify `handle_set_bracket_mode()`: when `entry_type == Limit`, create bracket with `entry.price = limit_price` (parsed from input) and `entry_type: EntryType::Limit`. Entry price does NOT track `last_price` for Limit orders.

**Key implementation details**:
- Order type tabs: `[Market] [Limit] [Stop] [Stop Limit]` — styled like the existing BUY/X/SELL toggle
- When switching from Limit back to Market, the entry price resets to `last_price`
- The `limit_price` input defaults to the current `last_price` when Limit tab is first selected
- In `MarketSnapshotLoaded` handler: only update draft bracket entry prices if `entry_type == Market`. For Limit/Stop/StopLimit brackets, preserve the user-specified entry price. Snapshot updates do not overwrite user-edited price inputs.
- **Type switching during drag**: If the user switches order type while mid-drag on the entry line (e.g., in `DraggingBracketLeg` mode), cancel the active drag and transition back to Normal interaction mode before applying the type change.

**Testing**:
- Unit: `SetEntryType(Limit)` sets state correctly
- Unit: switching Market → Limit → Market resets entry price to `last_price`
- Visual: select Limit → price input appears → enter price → toggle BUY → bracket appears at specified price

**Done when**: User can select Limit order type, enter a price, and see the bracket placed at that price with "LMT BUY @ {price}" label.

---

### Slice 3: Entry Line Dragging for Non-Market Orders
**Goal**: Make the entry line draggable for Limit/Stop/StopLimit brackets in Draft status. Drag updates the entry price and syncs to the order panel. Directional warnings guide the user but don't hard-block.
**Depends on**: Slice 2 (panel must have `entry_type`, `limit_price`, `stop_price` fields)
**Files to create or modify**:
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — modify `hit_test_bracket_legs()`: currently skips entry legs entirely. Add: if `bracket.entry_type != EntryType::Market` AND `bracket.status == Draft`, include the entry leg in hit testing. The entry leg uses `LegRole::Entry` and returns the entry price for grab-offset computation.
- `desktop/win/crates/midas-chart/src/interaction/mod.rs` — in the `DraggingBracketLeg` mouse-move handler: when `leg == LegRole::Entry`, emit `ChartAction::DragBracketLeg` with `leg: LegRole::Entry`. No side-constraint clamping on the entry leg (user can place at any price). The chart layer emits only the new price — it does NOT compute directional warnings (it has no access to bid/ask data).
- `desktop/win/crates/midas-app/src/app.rs` — in the `ChartDragBracketLeg` handler: remove the early-return guard that blocks entry leg updates. When `leg == LegRole::Entry`, update `bracket.entry.price = new_price`. Sync the order panel's `limit_price` (for Limit/StopLimit) or `stop_price` (for Stop) input field to match. Compute directional warnings here in the app layer (which has access to market data/bid-ask) and set a `wrong_side_warning: bool` flag on the bracket for the chart to render.

**Key implementation details**:
- Entry drag is allowed only for Draft brackets with `entry_type != Market`. Pending/Active brackets are not draggable (they're live at the broker).
- **Grab offset**: When hit-testing Entry legs, compute `grab_offset = bracket.entry.price - cursor_price`. In the drag handler, apply this offset directly: `new_price = cursor_price + grab_offset`. No clamping needed.
- **Directional warnings** (computed in app layer, not chart layer — chart has no bid/ask data):
  - Limit BUY: warn if dragged above current ask price (marketable limit)
  - Limit SELL: warn if dragged below current bid price
  - Stop BUY: warn if dragged below current bid price
  - Stop SELL: warn if dragged above current ask price
  - The app layer sets `wrong_side_warning: bool` on the bracket after processing the drag action. The chart renders warning amber on the entry label when this flag is true.
  - Warning state is a derived property of entry price vs. market price — it updates in real-time and clears automatically when the price moves to the correct side.
- When the entry price moves via drag, TP and SL legs do NOT move with it — they stay at their absolute prices.
- After drag, call `sync_draft_cache()` to keep the cache in sync.
- The `DraggingBracketLeg` mode's `entry_price` reference should be updated dynamically when dragging entry, so TP/SL constraints remain valid if the user subsequently drags TP/SL.

**Testing**:
- Unit: `hit_test_bracket_legs()` returns entry leg for Draft Limit bracket
- Unit: `hit_test_bracket_legs()` does NOT return entry leg for Draft Market bracket
- Unit: `hit_test_bracket_legs()` does NOT return entry leg for Pending Limit bracket
- Unit: directional warning emitted when Limit BUY dragged above ask
- Unit: no warning when Limit BUY dragged below ask
- Integration: drag entry line → price updates → label updates → cache synced → panel input synced

**Done when**: User can drag the entry line of a Draft Limit bracket to adjust the price. The label and order panel input update in sync. Directional warnings appear when price is on the "wrong" side.

---

### Slice 4: Stop and Stop-Limit Order Support
**Goal**: Extend the order panel and bracket creation for Stop and Stop-Limit entry types.
**Depends on**: Slice 3
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — add `stop_price: String` field to `OrderPanelState`, add `OrderPanelAction::SetStopPrice(String)` variant
- `desktop/win/crates/midas-app/src/app/views.rs` — when Stop selected, show stop price input (one field). When Stop Limit selected, show both stop price and limit price inputs (two fields). Market and Limit tabs unchanged.
- `desktop/win/crates/midas-app/src/app.rs` — modify `handle_set_bracket_mode()`:
  - Stop: create bracket with `entry.price = stop_price`, `entry_type: Stop`
  - StopLimit: create bracket with `entry.price = limit_price`, `entry_stop_price: Some(stop_price)`, `entry_type: StopLimit`
- `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs` — ensure `format_entry_label()` handles StopLimit dual-price display correctly

**Key implementation details**:
- For Stop orders, the entry line renders at the stop price (`entry.price`). This is the trigger price.
- For StopLimit, the entry line renders at the limit price (`entry.price`). The stop trigger price is shown in the label only (`entry_stop_price`). Rationale: the limit price is where the order actually executes, so it's the more useful visual reference.
- Stop-Limit dragging: dragging the entry line adjusts the limit price. The stop price is only adjustable via the panel input (for V1). Adding a second draggable line for the stop trigger is deferred.
- **StopLimit price validation**: For BUY, limit price must be ≤ stop price (the limit is the worst fill you'll accept after the stop triggers). For SELL, limit price must be ≥ stop price. If the user drags the limit past the stop, show a warning in the label.
- Default prices: Stop defaults to `last_price + 2%` (buy) or `last_price - 2%` (sell). StopLimit defaults stop to same, limit to the stop price.

**Testing**:
- Unit: bracket creation with Stop entry type — `entry.price = stop_price`
- Unit: bracket creation with StopLimit entry type — `entry.price = limit_price`, `entry_stop_price = Some(stop_price)`
- Unit: `format_entry_label()` for StopLimit — exact match: `"STP LMT BUY @ 185.00/184.50"`
- Unit: StopLimit price validation — warn when limit > stop for BUY
- Visual: select Stop Limit → two price inputs → toggle BUY → bracket appears

**Done when**: All 4 order types (Market/Limit/Stop/StopLimit) can be created from the order panel with correct labels, entry prices, and draggable entry lines. StopLimit price relationship is validated.

---

### Slice 5: Validation + Broker Bridge + Submission
**Goal**: Wire validated bracket submissions through to the broker engine for all order types.
**Depends on**: Slice 4
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/order_panel/mod.rs` — extend `validate_bracket()` to be type-aware: Limit orders must have `entry.price > 0`, Stop orders must have `entry.price > 0`, StopLimit must have both prices > 0 and `entry_stop_price` set, StopLimit BUY must have `limit_price ≤ stop_price`, StopLimit SELL must have `limit_price ≥ stop_price`. Confirm that the existing SL-vs-entry check ("stop loss must be below entry for BUY") remains correct for all entry types — it does, because for Stop BUY `entry.price` is above market and SL below that is valid, and for Limit BUY `entry.price` is at/below market and SL below that is also valid.
- `crates/midas-broker/src/orders/bracket/mod.rs` — rename `MarketBracketParams` to `BracketParams`, add `entry_kind: OrderKind`, `entry_price: Option<f64>`, and `entry_stop_price: Option<f64>` fields. The two price fields mirror `StopLossParams`' pattern (`stop_price` + `limit_price: Option`). Update all call sites directly — no type alias needed.
- `crates/midas-broker/src/engine/mod.rs` — rename `build_market_bracket()` to `build_bracket()`, update to set `order_type`, `limit_price`, `stop_price` on the parent `LocalOrder` based on `entry_kind`. Rename command variant `CreateMarketBracket` to `CreateBracket` and update all match arms across the codebase.
- `desktop/win/crates/midas-app/src/app.rs` — in `ChartBracketSubmit` handler: map chart `EntryType` → broker `OrderKind`, construct `BracketParams`, send `CreateBracket` command to broker via bridge. This replaces the current stub that only transitions to Pending.

**Key implementation details**:
- `EntryType → OrderKind` mapping (in app layer, at the bridge boundary):
  ```
  EntryType::Market → OrderKind::Market
  EntryType::Limit → OrderKind::Limit
  EntryType::Stop → OrderKind::Stop
  EntryType::StopLimit → OrderKind::StopLimit
  ```
  Note: `OrderKind::TrailingStop` is intentionally NOT mapped — it is a separate feature.
- For StopLimit, the broker's `LocalOrder` needs both `limit_price = entry.price` and `stop_price = entry_stop_price`
- **Bridge field mapping** (add as comment in bridge code):
  ```
  // Entry type → broker LocalOrder field mapping:
  //   Market:    limit_price = None,              stop_price = None
  //   Limit:     limit_price = entry_price,       stop_price = None
  //   Stop:      limit_price = None,              stop_price = entry_price
  //   StopLimit: limit_price = entry_price,       stop_price = entry_stop_price
  ```
- The existing `BrokerBridge::create_market_bracket()` method is renamed to `create_bracket()` accepting the new `BracketParams`
- Error handling: if broker rejects, bracket stays Draft, error shown in order panel status area (follow existing toast/error pattern)
- **Validation responsibility boundaries**: Chart-layer validation covers structural correctness (prices present, price relationships valid). App-layer validation covers UX warnings (wrong-side prices via `wrong_side_warning` flag). Broker-layer validation covers exchange/IB-specific constraints (deferred to Phase 1 IB integration).

**Testing**:
- Unit: `validate_bracket()` for Limit with valid price → pass
- Unit: `validate_bracket()` for Limit with zero price → fail
- Unit: `validate_bracket()` for StopLimit with missing stop price → fail
- Unit: `validate_bracket()` for StopLimit BUY with limit > stop → fail
- Unit: `BracketParams` construction for each entry type — correct `OrderKind` and prices
- Unit: `build_bracket()` sets correct `limit_price`/`stop_price` on `LocalOrder` for each entry type
- Integration: submit Limit bracket → broker receives correct `OrderKind::Limit` + `limit_price`

**Done when**: All 4 order types can be submitted to the broker with correct parameters. Validation catches invalid configurations. Broker rejection keeps bracket in Draft with error feedback.

### Dependency Summary

```
Slice 1 (data model + labels) ──→ Slice 2 (Limit UI + panel)
                                        │
                                        v
                                  Slice 3 (entry line dragging)
                                        │
                                        v
                                  Slice 4 (Stop + StopLimit UI)
                                        │
                                        v
                                  Slice 5 (validation + broker bridge)
```

- Linear dependency chain — each slice builds on the previous
- Critical path: 1 → 2 → 3 → 4 → 5
- Slice 1 is risk-free (data model only). Slice 3 is riskiest (new interaction pattern + drag constraints).
- Slices 1 & 2 could theoretically run in parallel (low file overlap), but sequential is cleaner for a single developer workflow.

## Risks & Unknowns

1. **Entry drag interaction complexity**: Making entry lines draggable reuses existing `DragBracketLeg` infrastructure, but the constraint logic is different (no clamping for entry, vs TP/SL must be on correct side). The grab offset computation for entry legs needs its own path (`grab_offset = bracket.entry.price - cursor_price`). Risk is moderate — the infrastructure exists, but entry-specific behavior needs careful testing.

2. **StopLimit dual-price display**: Showing two prices in one label ("STP LMT BUY @ 185.00/184.50") may be visually cluttered. Mitigation: can switch to two-line label or abbreviated format if needed. Exact format specified in Slice 1 tests as acceptance criteria.

3. **Broker bridge generalization**: Renaming `MarketBracketParams` and `CreateMarketBracket` affects the broker crate's public API. Mitigation: all call sites (including tests) are updated atomically in Slice 5's commit. No aliases needed — single-developer codebase with no external consumers.

4. **Entry price sync conflicts**: When switching from Limit to Market, the entry price snaps to `last_price`. When switching from Market to Limit, the entry price defaults to `last_price` as a starting point. These transitions need to feel smooth. The TP/SL legs remain at their absolute prices during type switching.

5. **rust-ibapi coverage**: The `OrderKind` enum exists but the actual IB submission path may have gaps for Stop-Limit. Mitigation: the test broker simulates all types; IB paper trading tests will be done in Phase 1 integration. The `EntryType → OrderKind` mapping explicitly excludes `TrailingStop` to prevent accidental misuse.

6. **StopLimit price relationship**: Users can drag the limit price past the stop price, creating an invalid order. Mitigation: directional warnings in the label and validation rejection at submission time (Slice 5).

7. **Stop price defaults**: The 2% default offset may produce poor defaults for illiquid stocks or during market gaps. For V1 this is acceptable — the user always adjusts via drag or input.

## Testing Strategy

- **Sans-IO unit tests** (midas-chart): `EntryType` enum, label formatting for all 4 types × 2 sides × 6 statuses (48 cases), entry hit-test filtering by type and status, serde backward compat
- **Drag constraint tests** (midas-chart): grab offset computation for entry legs, directional warning logic for all type/side combinations
- **Validation tests** (midas-app): `validate_bracket()` for all entry types with valid/invalid prices, missing stop prices, wrong-side limits, StopLimit price relationship
- **Broker tests** (midas-broker): `BracketParams` construction and `build_bracket()` for all 4 entry types, correct `OrderKind`/`limit_price`/`stop_price` mapping
- **Integration tests**: Full toggle→create→drag→submit flow for each order type
- **Type switching tests**: Market→Limit→Market entry price transitions, panel field sync during type changes

## Non-Goals / Out of Scope

- **Trailing Stop**: Separate feature with animated chart line. Deferred to Phase 2.
- **Market-on-Close / Limit-on-Close**: Auction order types, not needed for V1.
- **Conditional orders**: Price/time/volume conditions, deferred to Phase 3.
- **Two draggable lines for StopLimit**: V1 shows stop price in label only; second draggable line is a future enhancement.
- **Order modification after submission**: Modifying live orders at the broker is a separate feature (requires `ModifyOrder` command).
- **DOM (Depth of Market)**: Separate panel, not related to order type entry.
- **Chart reload bracket persistence**: Draft brackets are session-scoped via `draft_bracket_cache`. Persisting them across symbol switches is a separate enhancement.
- **Partial submission recovery**: If a bracket leg fails mid-submission to IB, recovery logic is deferred to Phase 1 IB integration work.

## Review Notes

The following trade-offs and alternatives were surfaced during the critique phase:

1. **Slice 1 & 2 parallelization**: These slices could theoretically run in parallel since their file overlap is limited. However, sequential execution produces a cleaner git history and avoids merge conflicts on `OrderBracket` constructors. Recommended: keep sequential unless working with multiple developers.

2. **`entry.price` semantic variation by type**: `entry.price` is the price the entry line renders at — for Market/Limit this is the target fill price, for Stop this is the trigger price (fill is approximate), for StopLimit this is the limit price. This means existing `risk_reward()`/`dollar_risk()`/`dollar_reward()` calculations are exact for Market/Limit and approximate for Stop (acceptable for V1). Mitigated by a per-type doc comment on `OrderBracket` (added in Slice 1) and an explicit field-mapping table in the bridge code (added in Slice 5).

3. **No hard blocks on entry drag**: The plan allows users to place limit orders on the "wrong" side of market (e.g., Limit BUY above ask). This matches TradingView's UX philosophy — warn but don't block. The alternative (hard clamping) was rejected because marketable limits are legitimate orders.

4. **Stop price defaults (2% offset)**: A simple hardcoded offset was chosen over configurable or ATR-based defaults. This is intentionally minimal for V1 — the user always has drag or input to override.
