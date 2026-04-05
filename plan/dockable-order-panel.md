# Feature: Dockable Order Panel

## Overview

Convert the modal order entry overlay into a first-class dockable pane (like Chart and Watchlist) that participates in the workspace layout, symbol linking, and config persistence. The panel is always visible, linked by `[S]` color groups, and supports multiple simultaneous instances for multi-symbol workflows. Follows the WatchlistPanel integration pattern exactly.

## Research Summary

### Codebase Analysis
- **Panel system**: `PanelContent` enum (`Chart | Watchlist`) in `layout.rs`, `PaneState` wraps it in `pane_grid::State`. Each type has: ID type in midas-core, HashMap on MidasApp, config struct, view functions.
- **AddWatchlist pattern** (`app.rs:2248`): `split()` always creates Chart → caller replaces `PaneState::content` → removes unwanted chart from `self.charts`. Order panel follows identical pattern.
- **Config persistence**: `walk_node()` pre-order traversal builds `LayoutNode` array; `restore_from_layout_tree()` rebuilds. Both need new `OrderPanel` variants.
- **Symbol linking**: `LinkMode` + `PickerTarget` enum + `find_link_targets()`. Watchlist has `[S]` button.
- **Current OrderPanelState**: Modal overlay with `visible: bool`, `source_chart: Option<ChartId>`. Gets symbol/price from focused chart. 15 Message variants.
- **Toolbar**: `views.rs:426` has `+` (AddChart) and `Watchlist` buttons. Order panel needs a button here.
- **midas-ui**: Has `TextButton`, `ButtonGroup`, `IconButton` but is NOT in midas-app's Cargo.toml. Can be added optionally but not required.

### Best Practices (Trading Platforms)
- ThinkOrSwim, NinjaTrader, TradeStation all use dockable order panels with color-coded link groups.
- On symbol change: **clear prices, keep quantity/side/TIF**. Offset-based TP/SL persist; absolute prices reset.
- Multiple simultaneous order panels is table-stakes for multi-instrument workflows.
- Always show current position for active symbol (future feature).
- Submit button should be disabled when market data not loaded.

### Idiomatic Approach
- Follow `WatchlistPanel` pattern: `OrderPanelId(u32)`, `PanelContent::Order(OrderPanelId)`, `HashMap<OrderPanelId, OrderPanel>`.
- Consolidate 15 Message variants into `OrderPanelMsg(OrderPanelId, OrderPanelAction)` with an action enum.
- ID allocation via monotonic counter on `WorkspaceLayout`, same as Chart/Watchlist.

## Design Decisions

### Decision 1: Message structure for multi-instance panels
**Context**: Current 15 `OrderPanel*` variants don't carry an ID. Multi-instance needs targeting.
**Options**:
1. Add `OrderPanelId` to each of the 15 variants — verbose, bloats Message
2. Single `OrderPanelMsg(OrderPanelId, OrderPanelAction)` with action enum
**Recommendation**: Option 2. Clean, scales to N panels.
**Confidence**: High

### Decision 2: What resets on symbol change
**Context**: When linked symbol changes, what persists vs resets?
**Recommendation**: Keep quantity, side, TP/SL enabled flags. Clear all price values (TP/SL values, last_price). Fetch new last_price from market_cache. Disable submit until last_price arrives.
**Confidence**: High

### Decision 3: Remove old modal panel
**Recommendation**: Remove entirely. Dockable panel replaces it. `T` key repurposed: focus nearest order panel or create one if none exists.
**Confidence**: Medium — may need user feedback

### Decision 4: Panel state struct
**Recommendation**: `OrderPanel { id, state: OrderPanelState, symbol_link: LinkMode }`. Minimal wrapper keeps form logic in `order_panel.rs` unchanged.
**Confidence**: High

## Implementation Plan

### Slice 1: Foundation types
**Goal**: Add `OrderPanelId`, extend `PanelContent`, `PanelSlot`, `LayoutNode`, and `AppConfig`.
**Depends on**: None
**Files to create or modify**:
- `desktop/win/crates/midas-core/src/id.rs` — add `OrderPanelId(u32)` with Display impl
- `desktop/win/crates/midas-core/src/lib.rs` — re-export `OrderPanelId`
- `desktop/win/crates/midas-core/src/config.rs` — add `OrderPanelConfig`, extend `PanelSlot`, `LayoutNode`, `AppConfig`
- `desktop/win/crates/midas-app/src/layout.rs` — add `PanelContent::Order`, `PaneState::order()`, `next_order_panel_id()`, `find_order_pane()`
**Key implementation details**:
- `OrderPanelId` follows `ChartId`/`WatchlistId` (Copy, Clone, Eq, Hash, Serialize, Deserialize, Display)
- `OrderPanelConfig { symbol: String, side: String, quantity: String, symbol_link: LinkMode }` — minimal for MVP, expand later
- `LayoutNode::OrderPanel { order_panel_index: usize }` indexes into `AppConfig::order_panels: Vec<OrderPanelConfig>`
- `LayoutNode` gains `#[serde(other)] Unknown` variant for forward compatibility — prevents hard deserialization failure if a future version removes a variant or an older binary encounters an unknown panel type
- `PanelSlot::OrderPanel { order_panel_index: usize }`
- `WorkspaceLayout` gains `next_order_panel_id: u32` initialized to 1
- Add temporary placeholder match arms (`PanelContent::Order(_) => { /* Slice 2/3 */ }`) in `view_content()` and `PaneClose` handler so the codebase compiles after Slice 1 alone. (Adding a new enum variant to an exhaustively matched enum is a hard error, not a warning.)
**Testing**:
- Unit test: OrderPanelId Display, OrderPanelConfig serde round-trip
- Unit test: `LayoutNode::Unknown` deserializes from an unrecognized `type` tag without error
**Done when**: Types compile. `cargo test -p midas-core` passes. `cargo check -p midas-app` passes (placeholder arms in place).

### Slice 2: OrderPanel struct + storage + add/close + toolbar
**Goal**: Create `OrderPanel` wrapper, store on MidasApp, add/close via messages, toolbar button.
**Depends on**: Slice 1
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/order_panel.rs` — add `OrderPanel` struct, `OrderPanelAction` enum, `to_config()`, `from_config()`
- `desktop/win/crates/midas-app/src/app.rs` — add `order_panels: HashMap<OrderPanelId, OrderPanel>`, `AddOrderPanel` + `OrderPanelMsg` messages, handlers
- `desktop/win/crates/midas-app/src/app/views.rs` — add "Order" button to toolbar
**Key implementation details**:
- `OrderPanel { id: OrderPanelId, state: OrderPanelState, symbol_link: LinkMode }`
- `OrderPanelAction` enum: `SetSide`, `SetQuantity`, `ToggleTp`, `SetTpMode`, `SetTpValue`, `ToggleSl`, `SetSlMode`, `SetSlValue`, `SetSlType`, `SetSlLimit`, `Submit`, `ConfirmYes`, `ConfirmNo`, `Dismiss`
- `AddOrderPanel` handler follows AddWatchlist:
  ```rust
  let op_id = self.workspace.next_order_panel_id();
  if let Some((chart_id, new_pane)) = self.workspace.split(Axis::Vertical, focused) {
      if let Some(state) = self.workspace.panes.get_mut(new_pane) {
          state.content = PanelContent::Order(op_id);
      }
      self.charts.remove(&chart_id);
      let symbol = self.active_chart_id()
          .and_then(|id| self.charts.get(&id))
          .map(|p| p.symbol.clone())
          .unwrap_or_default();
      self.order_panels.insert(op_id, OrderPanel::new(op_id, symbol));
      return self.flush_config();
  }
  ```
- `PaneClose` handler: add `PanelContent::Order(id)` arm — clean up `link_picker_open` if targeting this panel, remove from `self.order_panels`
- `LayoutPreset` handler (`app.rs:1578`): add order panel cleanup alongside existing chart/watchlist cleanup — remove orphaned entries from `self.order_panels` when preset is applied (presets only emit Chart panes)
- Toolbar: add `button(text("Order"))` next to "Watchlist" button
**Testing**:
- `OrderPanel::to_config()` / `from_config()` round-trip
**Done when**: Can add order panel via toolbar. Appears in grid (placeholder until Slice 3). Close removes it. Layout presets don't leak orphaned order panels.

### Slice 3: View rendering (title bar + body)
**Goal**: Render order form inside dockable pane.
**Depends on**: Slice 2
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app/views.rs` — add `PanelContent::Order` match arm, `view_order_title_bar()`, `view_order_body()`
**Key implementation details**:
- Title bar: symbol text + `[S]` link button + close `[×]`
- Body: port existing `view_order_panel()` overlay code into pane body:
  - Messages become `OrderPanelMsg(panel_id, action)` instead of `OrderPanelSetSide(side)`
  - Symbol from panel state, not from `source_chart`
  - `last_price` from `self.market_cache.get(&panel.state.symbol)`
  - Confirmation dialog renders inline (not as overlay)
  - Wrap body in `Scrollable` for narrow panes
- Disable submit when `last_price` is None (show "Market data loading..." instead)
- Remove the old modal overlay rendering (`view_order_panel()` function and its call site in `view()`)
**Testing**:
- Manual: add panel, form renders, interactions work
**Done when**: Order form renders in dockable pane. All interactions produce correct `OrderPanelMsg` messages.

### Slice 4: Symbol linking
**Goal**: Order panel participates in color-coded symbol linking.
**Depends on**: Slice 3
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/link.rs` — add `PickerTarget::Order(OrderPanelId)`
- `desktop/win/crates/midas-app/src/app.rs` — handle symbol propagation to/from order panels
**Key implementation details**:
- When linked chart/watchlist changes symbol, find order panels with matching LinkMode via `find_link_targets()`:
  ```rust
  let order_targets = find_link_targets(
      source_link,
      self.order_panels.iter().map(|(id, p)| (*id, p.symbol_link)),
  );
  for op_id in order_targets {
      if let Some(panel) = self.order_panels.get_mut(&op_id) {
          panel.state.symbol = new_symbol.clone();
          // Clear prices per Decision 2
          panel.state.tp_value.clear();
          panel.state.sl_value.clear();
          panel.state.sl_limit_value.clear();
          panel.state.last_price = None;
          panel.state.errors.clear();
      }
  }
  ```
- Trigger `load_market_snapshot()` for new symbol (same as watchlist add-ticker pattern)
- `ToggleLinkPicker` handler: add arm for `PickerTarget::Order(id)`
**Testing**:
- Manual: link order panel and chart with same color, click ticker, both update
**Done when**: Symbol propagation works. Form resets correctly per Decision 2.

### Slice 5: Config persistence
**Goal**: Order panels save/restore across sessions.
**Depends on**: Slice 2
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app/persistence.rs` — extend `walk_node()`, extend `build_config()`
- `desktop/win/crates/midas-app/src/app.rs` — extend `restore_from_layout_tree()`, update `RestoreCtx`
**Key implementation details**:
- `walk_node()`: `PanelContent::Order(id)` → call `panel.to_config()`, push to `order_panels` vec, push `LayoutNode::OrderPanel { order_panel_index }` to tree
- `restore_from_layout_tree()`: `LayoutNode::OrderPanel` → allocate `OrderPanelId`, `OrderPanel::from_config()`, insert into maps
- `RestoreCtx` gains `order_panels: HashMap<OrderPanelId, OrderPanel>` and `next_order_id: u32`
- ID allocation: sequential from 1, tracking highest seen, set `workspace.next_order_panel_id` at end
- `restore_from_layout_tree()`: handle `LayoutNode::Unknown` gracefully — skip the node (log warning) rather than crashing. This uses the `#[serde(other)]` variant added in Slice 1 for forward compatibility.
**Testing**:
- Integration test: construct a `layout_tree` Vec containing Split, Chart, Watchlist, and OrderPanel nodes, call `restore_from_layout_tree()`, assert correct panel types/count in the resulting workspace
- Manual: close and restart app. Order panels reappear with correct layout, symbol, quantity, side, link mode.
**Done when**: Full persistence round-trip works. Unknown layout nodes don't crash restoration.

### Slice 6: Wire order submission
**Goal**: Confirm button sends `CreateMarketBracket` to broker engine.
**Depends on**: Slice 3
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — `OrderPanelAction::ConfirmYes` handler
**Key implementation details**:
- Port `OrderPanelConfirmYes` logic: build `MarketBracketParams` from panel state, send to broker bridge, create annotation via `BrokerBracketCreated` self-message
- Add validation: if `panel.state.last_price.is_none()`, push error "Market data not loaded", return
- Keep reconciliation flow (local annotation → engine UUID reconciliation)
**Testing**:
- Place order from dockable panel. Bracket appears on chart. Broker processes it.
**Done when**: Full order flow works end-to-end.

### Slice 7: Clean up old modal panel
**Goal**: Remove old `OrderPanelState` singleton, overlay code, and 15 old message variants.
**Depends on**: Slices 3, 6
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — remove `order_panel: OrderPanelState` field, remove 15 old `OrderPanel*` message variants, remove old handlers
- `desktop/win/crates/midas-app/src/app/views.rs` — remove `view_order_panel()` overlay function
**Key implementation details**:
- Remove: `OrderPanelToggle`, `OrderPanelSetSide`, `OrderPanelSetQuantity`, `OrderPanelToggleTp`, `OrderPanelSetTpMode`, `OrderPanelSetTpValue`, `OrderPanelToggleSl`, `OrderPanelSetSlMode`, `OrderPanelSetSlValue`, `OrderPanelSetSlType`, `OrderPanelSetSlLimit`, `OrderPanelSubmit`, `OrderPanelConfirmYes`, `OrderPanelConfirmNo`, `OrderPanelDismiss`
- Remove `order_panel` field from `MidasApp` struct and construction
- Remove `view_order_panel()` and its call in `view()`
**Testing**:
- No dead code. All tests pass. App compiles clean.
**Done when**: Old modal code fully removed.

### Slice 8: T-key repurposing
**Goal**: `T` key focuses nearest order panel or creates one if none exist.
**Depends on**: Slice 7
**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — update `T` key handler
**Key implementation details**:
- Find first order panel pane via `workspace.find_order_pane()` (returns first match)
- If found: set focus to that pane
- If not found: dispatch `Message::AddOrderPanel` to create one
- Remove old T-key toggle logic
**Testing**:
- T with existing panel: focuses it
- T with no panels: creates one with focused chart's symbol
- T with no panels and no charts: creates one with empty symbol
**Done when**: T key works as specified.

### Dependency Summary
```
Slice 1: Foundation types
   |
Slice 2: OrderPanel struct + storage + add/close + toolbar
   |
   +------+
   |      |
  S3     S5
  (view) (config)
   |
   +------+
   |      |
  S4     S6
  (link) (broker)
   |
  S7 (cleanup) ← depends on S3 + S6
   |
  S8 (T-key)
```

**NOTE**: Slices 1+2 should land together (S1 alone produces exhaustive-match errors). S3 and S5 parallelize after S2. S6 depends on S3 (needs view for end-to-end testing). S4 depends on S3. S7 depends on S3 + S6. S8 depends on S7. Critical path: S1 → S2 → S3 → S6 → S7 → S8.

## Risks & Unknowns

### R1: Form layout in narrow panes
**Risk**: Order form designed as overlay may not adapt to narrow pane widths.
**Mitigation**: Wrap body in `Scrollable`. The form is already a vertical column that adapts to width. If too narrow, BUY/SELL buttons may wrap — acceptable.

### R2: Symbol change race with market data
**Risk**: last_price not in market_cache when symbol changes via link.
**Mitigation**: Show "--" for price, disable submit button, trigger `load_market_snapshot()`. Show validation error if user clicks submit before data arrives.

### R3: Annotation links lost on restart
**Risk**: `OrderAnnotationLink` is in-memory only. After restart, brackets have real broker UUIDs but the link map is empty.
**Mitigation**: This is a known limitation (documented in broker-bridge plan). Restored annotations are visual-only until order recovery feature (future). No change needed now.

## Testing Strategy

- **Unit tests**: `OrderPanelId` display, `OrderPanelConfig` serde, `OrderPanel::to_config()`/`from_config()`
- **Integration**: All existing 786 desktop tests pass unchanged
- **Manual per slice**: add panel, render, interact, link, persist, submit order, restart

## Out of Scope

- Price ladder / DOM panel
- Position display in order panel (needs broker account query)
- One-click trading mode (needs safety acknowledgment flow)
- BracketTool (3-click chart drawing) — separate feature
- Floating/pop-out order panels
- Order history display
- TP/SL offset mode persistence (add later if traders request)

## Review Notes

- **Critique agents flagged**: the old `view_order_panel()` function in views.rs line 1672 needs to be studied before Slice 3 to understand the exact form layout being ported. The new view should produce identical UI but with `OrderPanelMsg` messages instead of direct variants.
- **midas-ui widgets**: `TextButton` and `ButtonGroup` exist but aren't imported in midas-app. Consider adding the dependency in Slice 3 for cleaner BUY/SELL toggle, but not required.
- **PickerTarget::Order(OrderPanelId)**: follows the exact pattern of `PickerTarget::Watchlist(WatchlistId)`. No additional fields needed.
