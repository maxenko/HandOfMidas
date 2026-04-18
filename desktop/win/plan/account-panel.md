# Feature: Account Panel (tabbed Orders replacement)

## Overview

Consolidate per-account operations — positions, working orders, trade history, and symbol recall — into a single dockable **Account** pane so traders can monitor all account state side-by-side without fragmenting the workspace into multiple blotter panes. The refactor replaces the current `OrderBlotter` pane with a tabbed shell backed by the production-ready `midas_ui::Tabs` widget; each tab owns its own view-model and the shell is stateless apart from `active_tab`. Tab order and chrome follow the ThinkOrSwim reference.

The Orders tab preserves 100% of current blotter behaviour (14 columns, persistence, thumbnails, 8 state-machine tests). Positions is new and live-updates from `BrokerEvent::PositionUpdate` via a coalesced subscription. Trade History is a filtered view over the same `OrderBlotter` store. Recent Instruments is a bounded MRU tapped at the symbol-switch seam.

**Assumptions (verified during research):**

- `midas_ui::Tabs` is implemented at `desktop/win/crates/midas-ui/src/tabs.rs` with the exact API we need.
- `midas-ui` is NOT yet a dep of `midas-app`; this plan adds it.
- Account panel is **account-scoped** — all tabs show the full account view, matching the reference image (GME / AS / AAPL rows). Multi-account is out of scope; a docstring records the single-account assumption.
- `BrokerEvent::PositionUpdate { account, symbol, con_id, quantity, avg_cost }` exists in `crates/midas-broker/src/events.rs` but is NOT consumed in the app today. Derived fields (P/L, last_price, market_value) are computed inside `PositionStore`.
- `tokio-stream 0.1` is in `Cargo.lock` (transitive) but NOT in any `Cargo.toml` — Slice 4 adds it to `midas-app/Cargo.toml`.
- `OrderStatus::is_terminal()` does NOT exist yet — Slice 3 adds it.

## Research Summary

### Codebase findings

- **Orders panel** (`midas-app/src/order_blotter/{mod,panel,columns,persist}.rs`): `OrderBlotter` state machine with 8 tests (`mod.rs:480-703`); `OrderBlotterPanel` per-pane UI state (`panel.rs`); 14 columns (`columns.rs`); redb persistence with debounced flush (`persist.rs`).
- **Workspace panes** (`midas-app/src/layout/mod.rs`): `pane_grid::State<PaneState>`; `PanelContent` enum has `Chart | Watchlist | Order | OrderBlotter`. Factory methods on `PaneState`. Needs new `Account(AccountPanelId)` variant.
- **Toolbar** (`midas-app/src/app/views.rs:481-482`): "Orders" button → `Message::AddOrderBlotter` → handler at `handlers.rs:1712-1732`.
- **Broker subscription** (`midas-app/src/broker_bridge.rs:127-141`): `BrokerEventSource` wraps `broadcast::Sender<BrokerEvent>` with constant `Hash` impl — reuse directly.
- **Symbol-switch seam**: `app/handlers.rs:339` (`handle_panel_symbol_submitted`) and `app/ticker_wiring.rs:245-251` (`propagate_symbol_change`). Tap both for MRU.
- **Config**: `AppConfig` at `midas-core/src/config/mod.rs:39-80`; `OrderBlotterConfig` at `:254-270`; `PanelSlot` at `:348`; `LayoutNode` at `:380`. Both enums currently list `Chart | Watchlist | OrderPanel | OrderBlotter`. Need `Account` variants.
- **IDs**: `midas-core/src/id/mod.rs:59` generates `OrderBlotterId(u32)` via a shared macro. Follow the same pattern for `AccountPanelId(u32)`.
- **Dev harness injection**: `midas-app/src/dev_harness/broker_inject.rs` currently parses a fixed list of broker events; `PositionUpdate` is NOT supported. Slice 4 adds it.

### Best-practice findings

- **Stateful tabs** — preserve scroll/sort across switches. Each tab is its own struct on `AccountPanel`.
- **Coalesce broker bursts** — `tokio_stream::StreamExt::chunks_timeout(256, Duration::from_millis(50))` on the subscription; collapse to `Vec<PositionRaw>` per batch.
- **Cache badge counts** — update them in `update()` when the underlying store changes, never recompute in `view()`.
- **Stable `scrollable::Id`** — `format!("account-{panel_id}-{tab_name}")` so scroll survives tab switches AND is unique across panels.
- **Don't use `iced_aw::Tabs`** — rebuilds every tab's Element per frame. `midas_ui::Tabs` uses composition and sidesteps this.
- **Single disconnect banner** above the tab strip — don't decorate each tab individually.
- **Guard destructive UI at the handler level**, not just the button disable — user can reach the message via dev-harness.

## Design Decisions

### Decision 1: Rename-in-place over full replace

**Context**: Merge the Orders panel into Account while preserving the `OrderBlotter` row store.

**Options**: (1) Full delete + replace, (2) Keep both, (3) Rename pane type, preserve row store.

**Recommendation**: **Option 3**. `OrderBlotter` (row store + redb persistence) stays unchanged. Only the pane struct renames: `OrderBlotterPanel` → `AccountPanel` with a nested `OrdersTab` that holds the fields previously on `OrderBlotterPanel`. `PanelContent::OrderBlotter` is replaced by `PanelContent::Account`; legacy configs migrate to `AccountPanelConfig { active_tab: Orders, orders: inherited }` on load.
**Confidence**: high.

### Decision 2: Positions data model

**Context**: `BrokerEvent::PositionUpdate` carries only `{account, symbol, con_id, quantity, avg_cost}`; reference UI needs 10 columns including derived P/L fields.

**Recommendation**: `PositionStore { positions: HashMap<String, PositionRaw>, generation: u64 }` where `PositionRaw = { symbol, qty, avg_cost, last_price: Option<f32>, last_price_ts, session_open_price: Option<f32> }`. `last_price` receives from a price-tick stream (see Slice 4 probe task). Display layer derives `side = qty.signum()`, `market_value = qty.abs() * last_price`, `unrealized_pnl = qty * (last_price - avg_cost)`, `change_pct = (last_price - session_open_price) / session_open_price`. `realized_pnl` / `daily_pnl` render as em-dash until a broker-side `AccountPnlUpdated` event is plumbed.
**Confidence**: medium — Slice 4 probe task verifies the price stream source.

### Decision 3: Trade History = filtered view of OrderBlotter

**Context**: Separate store vs filter.

**Recommendation**: Filter `OrderBlotter` by `status.is_terminal()`. Zero new persistence; keeps Orders and History trivially consistent.
**Confidence**: high.

### Decision 4: Flat v1 config (incremental nesting later)

**Context**: Per-tab column-width persistence vs flat.

**Recommendation**: For v1, only Orders tab persists column widths / hidden columns (inherited from `OrderBlotterConfig`). Positions, History, Recents use fixed column widths stored runtime-only in their tab structs. When sort/filter ships (future), migrate to nested `TabConfig`.

```rust
// AccountPanelConfig schema (v1):
pub struct AccountPanelConfig {
    pub name: String,
    pub active_tab: AccountTab,           // "positions" | "orders" | "trade-history" | "recents"
    pub orders: OrdersTabConfig,          // structurally equal to former OrderBlotterConfig
}
pub struct OrdersTabConfig {
    pub column_widths: Vec<f32>,
    pub symbol_link: LinkMode,
    pub hidden_columns: Vec<String>,
}
```

All fields `#[serde(default)]` for forward-compat.
**Confidence**: high.

### Decision 5: Coalesced Positions subscription

**Context**: `PositionUpdate` at tick rate causes jank in iced's single-threaded `update`.

**Recommendation**: Subscription-level coalescing via `chunks_timeout(256, 50ms)` → `Message::Account(id, AccountMsg::PositionsBatchApplied(Vec<PositionRaw>))`. Single-event path (non-coalesced) for order events.
**Confidence**: high.

### Decision 6: Recents MRU ownership

**Context**: Where the list lives + how it's populated.

**Recommendation**: `MidasApp::recent_symbols: VecDeque<RecentEntry>` with `RecentEntry { symbol, last_seen: Instant }`. Persisted as `recent_symbols: Vec<String>` in `AppConfig` (symbols only; timestamps are in-memory, display falls back to "unknown" if loaded from disk). Tap points: `handle_panel_symbol_submitted` and `propagate_symbol_change`. Bounded at 20; dedup moves-to-front.
**Confidence**: high.

### Decision 7: Versioned config migration

**Context**: Legacy `order_blotters` entries must become `account_panels` without data loss.

**Recommendation**: Pure function `fn migrate_order_blotters_to_account_panels(cfg: &mut AppConfig)` in `midas-core/src/config/migrations.rs` (new file). Called unconditionally at `AppConfig::load`; idempotent (no-op if `order_blotters` empty). Before overwriting `config.toml` during the first save after migration, copy to `config.toml.bak-account-migration`. Log migration event via `tracing::info!`.
**Confidence**: high.

## Implementation Plan

### Slice 1: Account pane + Orders tab (vertical, replaces current Orders panel end-to-end)

**Goal**: Toolbar button reads "Account", clicking it opens a dockable pane with four tabs; the Orders tab renders identically to today's Orders panel (14 columns, persistence, thumbnails, row select, sort, hide-column popup). Positions/History/Recents tabs show empty-state placeholders. First user-visible slice, end-to-end testable.

**Depends on**: None.

**Files to create or modify**:
- `desktop/win/crates/midas-core/src/id/mod.rs` — add `AccountPanelId(u32) => "Account"` to the id-macro invocation list.
- `desktop/win/crates/midas-core/src/lib.rs:32` — re-export `AccountPanelId`.
- `desktop/win/crates/midas-core/src/config/mod.rs` — add `account_panels: Vec<AccountPanelConfig>` to `AppConfig` (with `#[serde(default)]`); keep `order_blotters` for migration input; add `PanelSlot::Account { account_panel_index: usize }` and `LayoutNode::Account { account_panel_index: usize }` variants; add `AccountPanelConfig` and `OrdersTabConfig` structs; `AccountTab` enum with `#[serde(rename_all = "kebab-case")]`.
- `desktop/win/crates/midas-core/src/config/migrations.rs` (new) — `migrate_order_blotters_to_account_panels()`; unit-tested with round-trip.
- `desktop/win/crates/midas-core/src/config/mod.rs` `AppConfig::load` — call migration; if migration ran, back up `config.toml` to `config.toml.bak-account-migration` via `std::fs::copy` before the next save.
- `desktop/win/crates/midas-app/Cargo.toml` — add `midas-ui = { path = "../midas-ui" }`.
- `desktop/win/crates/midas-app/src/layout/mod.rs` — add `PanelContent::Account(AccountPanelId)` variant; add `PaneState::account(AccountPanelId)` factory; update all `match` arms in the file; add `WorkspaceLayout::next_account_panel_id()` counter.
- `desktop/win/crates/midas-app/src/account_panel/mod.rs` (new) — struct `AccountPanel { id, name, active_tab, orders: OrdersTab }`; enum `AccountMsg { TabSelected(AccountTab), Orders(OrderBlotterGridMsg), /* tabs for later slices */ }`; `view(&self, theme: &UiTheme, blotter: &OrderBlotter, recent_symbols: &[RecentEntry], positions: &PositionStore, broker_connected: bool) -> Element<'_, AccountMsg>`.
- `desktop/win/crates/midas-app/src/account_panel/orders_tab.rs` (new) — `OrdersTab` struct with the fields currently on `OrderBlotterPanel` (`grid_state`, `symbol_link`, `hidden_columns`, `last_seen_generation`, `selected_row`); `from_config`/`to_config` mirror `OrderBlotterPanel::{from_config, to_config}` at `order_blotter/panel.rs:49-95`.
- `desktop/win/crates/midas-app/src/app.rs` — add `account_panels: BTreeMap<AccountPanelId, AccountPanel>` field; drop any direct references to `order_blotters` from `MidasApp` (kept only in config for migration reading).
- `desktop/win/crates/midas-app/src/app/views.rs:481-482` — change button text "Orders" → "Account"; change message `AddOrderBlotter` → `AddAccountPanel`.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — add `handle_add_account_panel` (mirror `handle_add_order_blotter:1712-1732`); route `Message::Account(AccountPanelId, AccountMsg)` to the relevant panel.
- `desktop/win/crates/midas-app/src/app/persistence.rs` — in `build_config`, emit `AccountPanelConfig` for each `PanelContent::Account`; drop code paths that emit `OrderBlotterConfig` (the migration handles one-time transition on first read).
- `desktop/win/crates/midas-app/src/order_blotter/panel.rs` — delete `OrderBlotterPanel`. Keep the module file only for convenience re-exports if needed; `columns.rs`/`mod.rs`/`persist.rs` untouched.

**Key implementation details**:
- `AccountPanel::view` signature is fixed (see new file spec above) — callers pass the slices they need; avoids `&MidasApp` god-parameter.
- Scrollable IDs: `scrollable::Id::new(format!("account-{}-orders", self.id.as_u32()))` pattern; same for empty-state placeholders in the other three tabs (ensures uniqueness across panels).
- Tab construction in `AccountPanel::view`:
  ```rust
  Tabs::new(
      vec![
          TabItem::new("Positions", AccountTab::Positions),
          TabItem::new("Orders", AccountTab::Orders)
              .with_badge(working_order_count(blotter)),
          TabItem::new("Trade History", AccountTab::TradeHistory),
          TabItem::new("Recent Instruments", AccountTab::Recents),
      ],
      self.active_tab,
      AccountMsg::TabSelected,
  )
  .view(theme)
  ```
  Positions/History/Recents badges appear once their slices land.
- Migration test fixture: a `config.toml` with one `[[order_blotters]]` entry round-trips to exactly one `[[account_panels]]` with `active_tab = "orders"` and the blotter's column widths + hidden columns + name preserved.

**Probe task (must complete before Slice 1 code)**:
- Verify `propagate_symbol_change` signature and call sites; record in Slice 2's spec.
- Verify `BrokerEvent::PriceTick` or equivalent last-price variant; record in Slice 4's spec.
- Confirm `OrderStatus::is_terminal()` absence (add in Slice 3).

**Testing**:
- Unit: config migration round-trip (one blotter → one account panel, widths preserved).
- Unit: `AppConfig::load` with a pre-existing `config.toml` containing `[[order_blotters]]` produces a migrated `[[account_panels]]` and writes a `.bak-account-migration` file.
- Unit: `OrdersTab::from_config`/`to_config` round-trip (column widths, hidden columns, link mode).
- All 8 existing `OrderBlotter` state-machine tests stay green (row store untouched).
- Manual (devloop): boot app, click "Account" button, pane appears; switch tabs, underline moves; Orders tab shows existing orders; restart app, Account panel restored with same active tab.

**Done when**: Visual parity with the current Orders panel on the Orders tab; all other tabs show empty-state; clicking a tab updates the underline; closing and reopening preserves active tab and column widths; `config.toml.bak-account-migration` exists after first-run migration.

### Slice 2: Recent Instruments tab

**Goal**: MRU list of recently-selected symbols tracked in `MidasApp`, persisted in `AppConfig`, displayed in the Recents tab; clicking a row sets the focused chart's symbol.

**Depends on**: Slice 1 only.

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/app.rs` — add `recent_symbols: VecDeque<RecentEntry>` field with `MAX_RECENTS = 20`; `push_recent_symbol(&mut self, symbol: &str)` helper with dedup-move-to-front and cap.
- `desktop/win/crates/midas-app/src/app/handlers.rs:339` — call `push_recent_symbol` at entry of `handle_panel_symbol_submitted`.
- `desktop/win/crates/midas-app/src/app/ticker_wiring.rs:245-251` — call `push_recent_symbol` inside `propagate_symbol_change`.
- `desktop/win/crates/midas-core/src/config/mod.rs` — add `recent_symbols: Vec<String>` (symbols only) to `AppConfig` with `#[serde(default)]`.
- `desktop/win/crates/midas-app/src/account_panel/recents_tab.rs` (new) — `RecentsTab` (stateless; reads from `MidasApp::recent_symbols`); view is a `column!` of tappable rows; badge = `min(count, 99)`.
- `desktop/win/crates/midas-app/src/account_panel/mod.rs` — wire Recents tab view; add `AccountMsg::RecentClicked(String)`.
- Handler for `AccountMsg::RecentClicked(symbol)` → call existing `propagate_symbol_change` on the focused chart (if any); no-op if no chart focused (log via tracing).

**Key implementation details**:
- `RecentEntry { symbol: String, last_seen: Instant }` — `Instant` is runtime-only; if the struct is reconstructed from config at startup, `last_seen = None` and the display falls back to "—".
- Display row: `button(row![text(&entry.symbol), horizontal_space(), text(format_elapsed(entry.last_seen))])` — `format_elapsed` returns strings like "3m ago", "12h ago", "—" (None case).
- Empty state: centered Label — "No recent instruments — switch a chart's symbol to populate".

**Testing**:
- Unit: `push_recent_symbol` dedup moves to front; cap enforced; order preserved.
- Unit: `AppConfig` round-trip includes `recent_symbols`.
- Unit: `RecentsTab::view` in empty state shows the placeholder.
- Manual (devloop): set chart to MSFT, TSLA, AAPL in turn; Recents tab shows [AAPL, TSLA, MSFT]; click TSLA, chart switches.

**Done when**: Recents tab populates on symbol switches; clicking a row re-selects the symbol on the focused chart; list survives restart (symbols only); badge shows count (hidden when 0).

### Slice 3: Trade History tab (filter over OrderBlotter)

**Goal**: Read-only tab showing all terminal orders with timestamp, symbol, side, qty, fill price, status columns. Row-level actions: none.

**Depends on**: Slice 1 only.

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/order_blotter/mod.rs` — add `impl OrderStatus { pub fn is_terminal(&self) -> bool { matches!(self, Self::Filled | Self::Cancelled | Self::Rejected) } }`. Add a similar helper `OrderBlotter::terminal_row_count(&self) -> usize` cached on the blotter (invalidated with `generation`); this feeds the History-tab badge without iterating every `view()`.
- `desktop/win/crates/midas-app/src/account_panel/history_tab.rs` (new) — `HistoryTab { grid_state, selected_row, last_seen_generation }`; `DisplayRow` precomputed per generation from filtered blotter rows; fixed sort by `last_update_at` descending.
- `desktop/win/crates/midas-app/src/account_panel/history_columns.rs` (new) — 6 column defs: `Timestamp`, `Symbol`, `Side`, `Qty`, `FillPrice`, `Status`. Pattern: `order_blotter/columns.rs:165-472`. Use fixed widths (v1 does not persist History widths).
- `desktop/win/crates/midas-app/src/account_panel/mod.rs` — wire History tab; badge = `blotter.terminal_row_count()`.

**Key implementation details**:
- Empty state: "No trade history yet".
- Badge shows only when count > 0.
- Row click: no-op (read-only v1); document in module docstring.

**Testing**:
- Unit: `OrderStatus::is_terminal` truth table.
- Unit: `terminal_row_count` matches manual filter; stale after mutation until generation bumps.
- Unit: `HistoryTab::rebuild_rows` filters correctly (Working excluded; Filled/Cancelled/Rejected included); sort is desc by time.
- Manual (devloop): inject a filled bracket, History shows 3 rows with matching status tints.

**Done when**: History tab shows all terminal orders sorted most-recent-first; badge reflects count; column set matches the reference image (6 columns); no persistence of History widths in v1.

### Slice 4: Positions subscription + disconnect banner (plumbing, no grid)

**Goal**: Wire the coalesced broker-event subscription feeding `PositionStore`; display the disconnect banner above the tab strip; verify via tracing logs that live updates arrive. Positions tab still shows "Loading…" placeholder — no grid yet.

**Depends on**: Slice 1. Contains the riskiest plumbing work; fails fast if broker integration has gaps.

**Files to create or modify**:
- `desktop/win/crates/midas-app/Cargo.toml` — add `tokio-stream = "0.1"` (currently transitive via broker; elevate to direct dep).
- `desktop/win/crates/midas-app/src/account_panel/positions_store.rs` (new) — `PositionStore { positions: HashMap<String, PositionRaw>, generation: u64 }`; `apply(&mut self, update: &PositionUpdate)`; `apply_batch(&mut self, batch: &[PositionRaw])`; `update_last_price(&mut self, symbol: &str, price: f32)`. Removes a symbol when incoming `qty == 0`.
- `desktop/win/crates/midas-app/src/account_panel/subscription.rs` (new) — `fn positions_subscription(source: BrokerEventSource) -> Subscription<AccountBatchMsg>` using `BroadcastStream::new(source.sender.subscribe()).filter_map(..PositionUpdate..).chunks_timeout(256, Duration::from_millis(50)).map(fold_latest_per_symbol)`.
- `desktop/win/crates/midas-app/src/app.rs` — add `positions: PositionStore` field; extend `subscription()` to include `positions_subscription`; wire `Message::Account(_, AccountMsg::PositionsBatchApplied(batch))` handler.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — also consume `BrokerEvent::PositionUpdate` in the existing single-event path for accounts where coalescing isn't needed (e.g., during backfill). Guard idempotency.
- `desktop/win/crates/midas-app/src/dev_harness/broker_inject.rs` — parse `"PositionUpdate"` variant and emit `BrokerEvent::PositionUpdate { account, symbol, con_id, quantity, avg_cost }`; add to the variant dispatch list; add a unit test.
- `desktop/win/crates/midas-app/src/account_panel/mod.rs` — `view()` renders a `DisconnectBanner` container above the Tabs if `!broker_connected`; banner text: "Disconnected — data may be stale". Does NOT auto-dismiss on reconnect per TWS convention; has an "×" acknowledge button that clears a session-scoped `disconnect_banner_ack: bool` on the panel.
- `desktop/win/crates/midas-ui/src/theme.rs` — add `warning_bg: Color` and `warning_text: Color` fields (dark-theme defaults: amber). `#[serde(default)]` for forward-compat.

**Probe (at the top of Slice 4)**:
- Confirm the last-price stream: search for `BrokerEvent::PriceTick`, `BrokerEvent::Tick`, or equivalent; record the variant name and plumbing in this slice's impl notes before writing subscription code.
- Confirm `BrokerEvent::AccountPnlUpdated` exists or doesn't; record result in this slice's notes. If absent: `realized_pnl` / `daily_pnl` show em-dash in Slice 5 (already the plan).

**Key implementation details**:
- `fold_latest_per_symbol(batch: Vec<PositionUpdate>) -> Vec<PositionRaw>`: walk `batch` once, keep latest by `(account, symbol)`.
- Subscription filter: ignore events for accounts other than the app's configured account (record TODO if multi-account lands later).
- Load test: dev-harness smoke script injecting `PositionUpdate` at 200/sec for 10 s; verify no dropped batches (tracing counter) and no frame drops (event-log timestamps cluster within 50ms windows).

**Testing**:
- Unit: `PositionStore::apply` — insert, overwrite, removal on qty=0.
- Unit: `fold_latest_per_symbol` — two updates to same symbol in a batch collapse to one.
- Unit: `broker_inject::parse` accepts `PositionUpdate` and rejects malformed payloads.
- Unit: disconnect banner rendered iff `!broker_connected && !disconnect_banner_ack`.
- Manual (devloop): inject a `PositionUpdate` via dev-harness, verify `positions` map updates; simulate disconnect and verify banner appears.

**Done when**: `cargo test` green; devloop can inject synthetic positions; banner renders and dismisses; no broker events dropped at 200/s sustained; Positions tab still shows "Loading…" (grid comes in Slice 5).

### Slice 5: Positions grid + close-position stub

**Goal**: Live-updating Positions grid with 10 columns plus close-X action; destructive-action safety guards enforced both at UI and handler level.

**Depends on**: Slice 4.

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/account_panel/positions_tab.rs` (new) — `PositionsTab { grid_state, selected_row, last_seen_generation }`; `DisplayRow` derives 10 fields from `PositionRaw` and app-level state. Close-X cell renders `IconButton::new("×")` (or similar from midas-ui) with `on_press(AccountMsg::Positions(PositionsMsg::CloseRequested(symbol)))`.
- `desktop/win/crates/midas-app/src/account_panel/positions_columns.rs` (new) — 10 column defs: `Symbol`, `Side`, `Qty`, `AvgPrice`, `LastPrice`, `ChangePct`, `UnrealizedPnl`, `RealizedPnl`, `DailyPnl`, `MarketValue`, plus trailing `CloseAction`. Side tint (blue/red) matches `order_blotter/columns.rs` pattern.
- `desktop/win/crates/midas-app/src/app/handlers.rs` — handler for `PositionsMsg::CloseRequested(symbol)`: **return early** if `!self.broker.is_connected()`, log via tracing, set `self.status_message = "Disconnected — close unavailable"`; else log intent and set status message. **No broker command is sent in v1.**
- `desktop/win/crates/midas-app/src/account_panel/mod.rs` — wire Positions tab content when `active_tab == Positions`; badge = open-position count (`positions.positions.len()`).

**Key implementation details**:
- Sign convention: `qty > 0` Long, `qty < 0` Short, `qty == 0` row removed by store.
- Em-dash fallbacks for unavailable fields (match Decision 2).
- Close-X button visual: rendered as disabled (40% alpha) when `!broker_connected`; tooltip reads "Disconnected — close unavailable".
- Asserting handler safety: a unit test for the `CloseRequested` handler verifies no `BrokerCommand` is constructed, regardless of connection state. This locks in the stub-only guarantee.

**Testing**:
- Unit: `PositionRow::derive` — side sign, unrealized P/L signs, market_value, change_pct with and without session_open_price.
- Unit: `CloseRequested` handler test — asserts no `BrokerCommand::Submit*` is emitted; when disconnected, status message is set.
- Manual (devloop): inject two synthetic `PositionUpdate`s (Long GME, Short AS); grid renders correct sign tints and derived values; click close-X while connected, status message logs intent; disconnect, close-X is disabled.

**Done when**: Positions tab renders live rows; close-X renders but only logs; safety guards enforced at handler level; badge shows count; empty-state when 0 positions; disconnect banner still present from Slice 4.

### Slice 6: Polish (empty-state consistency + clippy)

**Goal**: Cohesive empty-state visuals; all new code passes `cargo clippy --workspace -- -D warnings`; reference-image parity at the pixel level.

**Depends on**: Slices 1–5.

**Files to create or modify**:
- `desktop/win/crates/midas-app/src/account_panel/empty_state.rs` (new) — `fn empty_state(message: &str, theme: &UiTheme) -> Element<'_, AccountMsg>`; 14pt centered label at 40% alpha.
- Replace inline placeholders in Positions/Orders/History/Recents tabs with `empty_state(..)`.
- Clippy/fmt pass across all new modules.

**Testing**:
- `cargo clippy --workspace -- -D warnings`.
- Manual (devloop): screenshot each tab in empty state; compare side-by-side against reference image.

**Done when**: Empty-state styling identical across tabs; clippy clean; visual parity confirmed.

### Dependency summary

- **Critical path**: 1 → 4 → 5 → 6. 
- **Parallelizable**: Slices 2 and 3 run independently after Slice 1.
- **Recommended agent allocation**: Slice 1 (single agent, sequential). Slices 2 + 3 + 4 in parallel (three isolated worktrees). Slice 5 after 4. Slice 6 last.

## Risks & Unknowns

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `BrokerEvent::AccountPnlUpdated` not present | High | Realized/Daily P/L show em-dash | Accept as v1; schedule broker follow-up |
| Last-price stream name differs from assumptions | Medium | Slice 4 probe blocked | Probe task at top of Slice 4 records actual variant before coding |
| Config migration loses user's custom-named blotter | Low | Panel named differently | `name` field preserved through migration; backup file is recoverable |
| `tokio-stream` version mismatch with broker-crate pin | Low | Build break | Use `"0.1"` loose pin; integration CI catches drift |
| Sustained broker event storm (>200/s) starves iced | Medium | UI stutter | Load test in Slice 4 validates 200/s; bump `chunks_timeout` cap to 512 if needed |
| Multi-account scenario not considered | Low | Wrong positions shown across accounts | v1 assumes single account; filter by `event.account` to most-recently-seen; document assumption |
| Close-position stub mistakenly submits an order | Medium | Unwanted broker call | Handler-level `is_connected()` guard + unit test asserting no `BrokerCommand` emitted |
| Scrollable ID collision between multiple Account panels | Low | Shared scroll state | Include `AccountPanelId` in `scrollable::Id` string |
| Config backup fails (disk full, permission) | Low | No backup, still risky migration | `?`-propagate the copy error; abort migration if backup fails; log loudly |

## Testing Strategy

- **State machines**: pure unit tests in `#[cfg(test)] mod tests` (no iced). Pattern: `order_blotter/mod.rs:480-703`.
- **Config round-trip**: `midas-core/src/config/mod.rs` pattern — serialize, deserialize, assert equality.
- **Migration**: unit test with fixture TOML strings.
- **Subscription coalescing**: `tokio::time::pause()` + in-process `broadcast::Sender` to assert batching.
- **Handler safety**: pure unit tests on handler functions asserting absence of destructive `BrokerCommand`.
- **Visual**: dev-harness smoke per `scripts/devloop-smoke.sh`, screenshot + ref-image comparison per tab.
- **No iced view-snapshot tests** — project does not use them.

## Non-Goals / Out of Scope

- Column sort/filter beyond Orders tab.
- Round-trip (entry+exit) P/L on History tab.
- Multi-account workspace.
- Keyboard navigation across tabs.
- Animated underline between tab switches.
- Close-position wired to broker (stub only v1).
- Per-symbol context menu on Positions rows.
- Drag-to-reorder tabs / tab close buttons / overflow menu.

## Review Notes

- **Slice reordering**: Originally six slices had Positions last (Slice 5). Critique (Agent C) flagged that this defers the riskiest work. This plan moves subscription plumbing + disconnect banner to Slice 4 (before the grid), so coalescing/broker-integration risk surfaces after the first feature-visible slice.
- **Config schema**: Agent C recommended a flat v1 schema instead of per-tab `TabConfig` nesting. Adopted. Only Orders persists column widths in v1; future slices can add per-tab configs without breaking existing files (all new fields `#[serde(default)]`).
- **Disconnect banner** is NOT polish — it's a precondition for the close-position stub's safety. Moved from Slice 6 to Slice 4.
- **Slice 1 was originally horizontal** (scaffolding + empty states). Folded the Orders tab into Slice 1 so the first slice delivers end-to-end functional parity with today's Orders panel.
- **Tokio-stream** is a transitive dep in `Cargo.lock` but not a direct dep of `midas-app`; Slice 4 adds it explicitly.
- **Handler-level safety** on close-position (not just UI disable) is a non-negotiable per CLAUDE.md rule #3 (live-trading guard).
