# Multi-Named-Window Support for Hand of Midas

## Context

Today, only the main window in `midas-app` has a full docking layout. Charts can be popped into single-purpose floating windows (`floating_charts: HashMap<window::Id, ChartPanel>`, plus the feature-gated `floating_session_charts`), but those popouts can host nothing else and are not persisted.

The user wants every window to be as capable as the main window:

1. Host any panel mix (charts, watchlists, order entry, account, blotter).
2. Full docking / pane-grid layout.
3. Independent panel adding from inside that window.
4. Layout + geometry persisted in config and reloaded next launch.
5. Each window keyed under its user-chosen name.
6. Free-form user-supplied window names ("My Intraday Trades").

The runtime is already on `iced::daemon()` (see `midas-app/src/main.rs:50-107`, iced 0.14 per `desktop/win/Cargo.toml`), so multi-window is a refactor of state ownership, message routing, and config schema — not a new windowing primitive.

### User-confirmed design choices

- **Ticker state stays shared per symbol.** `TickerState`, `AnnotationStore`, and `ChartViewStore` remain keyed by `SymbolKey`. AAPL is AAPL across windows.
- **Retire the legacy popouts.** `floating_charts` and `floating_session_charts` are removed. "Pop out chart" becomes "Open in new named window" with a single-pane layout.
- **Single config.toml.** `AppConfig` bumps to v3 with `windows: BTreeMap<String, WindowConfig>`. Atomic save and debounced flush stay as today.
- **All panel kinds in any window.** Charts, watchlists, order entry, account, blotter all hostable in any named window via the unified pane grid.

## Goals

1. Open arbitrary user-named windows alongside the main window, each with a full pane-grid layout that can host any panel kind (chart, watchlist, order entry, account, blotter).
2. Independently add panels to a focused window via the same UX surfaces as today (`Ctrl+N`, header `Add ▾`).
3. Persist every window's geometry, name, and layout in `config.toml` and restore on next launch.
4. Replace the legacy chart-popout (`floating_charts`, `floating_session_charts`) with a unified "open in new named window" path.
5. Extend the dev harness to drive multiple windows by name (screenshot, click, key, fixture).

## Non-Goals

- **Cross-window pane drag-and-drop.** iced 0.14's `pane_grid::on_drag` is per-state. Moving a panel between windows uses the popout path or a context menu, not native drag.
- **Workspace presets / saved closed-window layouts.** Closing a non-main window destroys its config entry; "save and reopen by name later" is a future feature.
- **Per-window broker connection or per-window theme.** Broker connection state, theme, and ticker bar remain global (main window only).
- **Per-window TickerState / AnnotationStore divergence.** Confirmed user choice — symbol-level state is global.
- **Per-window status bar.** Connection LED / status text live in main only.
- **User-driven "move panel to window N" UI.** Dev-harness `MoveChartToWindow` lands; user-facing equivalent is a follow-up if asked.
- **Multi-monitor restore beyond "fall back to primary on missing display".** No per-monitor anchoring or DPI-aware repositioning beyond what today's `WindowGeometryConfig` already does.

---

## Cross-plan alignment

Two parallel feature plans land alongside this one and touch some of the same surfaces:

- `plan/session-aware-charts/eth-shading.md` — adds nested fields to `ChartConfig` (`show_extended_hours`, `show_extended_hours_bands`) and 4 fields to `ChartInput`. Promotes three small crates to baseline `midas-app` deps. Independent of windowing.
- `plan/volume-profile-anchored/00-index.md` — adds `chart.volume_profile` (nested) to `ChartConfig` and `experimental.disable_anchored_vp` to `AppConfig`. Adds a devloop `SetVpSettings` command.

See `plan/cross-plan-alignment.md` for the full touchpoint matrix. Specifically for this plan:

- The v2→v3 migration's leaf-rewrite (`LayoutNode::Chart {chart_index}` → `{chart_id}`) is invisible to the other two plans (they don't reference `LayoutNode`). The migration also does NOT touch nested `ChartConfig` fields or top-level `[experimental]`, so ETH's and VP's serde-default fields flow through cleanly regardless of which lands first.
- Slice G's `window: Option<String>` convention on devloop commands is the canonical pattern; VP's `SetVpSettings` and any future ETH harness commands MUST adopt it (either at landing time or in a follow-up commit if multi-window lands second).
- Slice F1/F2 (popout retirement) renders VP's "Floating session-chart preset windows" non-goal moot — those become regular named windows inheriting VP and ETH visuals via the standard per-chart config path.
- Slice F2's gated spike (folding `session_chart_window.rs` into a `pane_grid` cell) does NOT touch `session_chart::scene_builder`, which is where VP S3 wires `VolumeProfileLayer`. VP S3 composes whether F2 lands or stays deferred.

---

## Architecture

### New types

```rust
// midas-core/src/window_key.rs (new)
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug)]
#[serde(transparent)]
pub struct WindowKey(Arc<str>);

impl WindowKey {
    pub const MAIN_DEFAULT: &'static str = "Main";
    pub fn new(s: impl Into<Arc<str>>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn normalize(s: &str) -> Result<Self, NameError> {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.len() > 64 { return Err(...); }
        Ok(Self(Arc::from(trimmed)))
    }
}
```

The key **is** the user-visible name. Renaming = remove-and-reinsert in `BTreeMap`. `Arc<str>` keeps clones cheap (used in many message-construction closures).

```rust
// midas-app/src/app/window_state.rs (new)
pub struct WindowState {
    pub key: WindowKey,
    pub is_main: bool,
    pub iced_id: Option<window::Id>,   // None until window::open's Task resolves
    pub layout: WorkspaceLayout,        // moved out of MidasApp.workspace
    pub geometry: WindowGeometry,       // per-window; was singleton
    pub opening: bool,                  // suppresses geometry events during open
}
```

```rust
// midas-app/src/app/panel_ids.rs (new)
pub struct PanelIdAllocator {
    next_chart: u32,
    next_watchlist: u32,
    next_order_panel: u32,
    next_account_panel: u32,
}
// All `next_*` methods return a fresh ID. Counters move OUT of WorkspaceLayout
// because two windows would otherwise mint colliding IDs into MidasApp's
// global panel maps.
```

### `MidasApp` field changes

```rust
pub struct MidasApp {
    // ENDS UP REPLACING: workspace, floating_charts, floating_session_charts, window.
    // During slices A1–E, `windows` lives ALONGSIDE the legacy fields; slice F1 deletes
    // `floating_charts`, slice F2 (gated on a spike) deletes `floating_session_charts`.
    // See "Phased slices" for the transitional state at each boundary.
    pub windows: BTreeMap<WindowKey, WindowState>,
    pub iced_id_to_key: HashMap<window::Id, WindowKey>,  // reverse lookup
    pub main_window_key: WindowKey,                       // cached "which key is main"
    pub focused_window: Option<WindowKey>,                // last OS-Focused window
    pub pending_window_opens: HashMap<window::Id, WindowKey>, // until WindowAttached
    pub panel_ids: PanelIdAllocator,                      // app-global, see above
    // panel_to_window: PanelId → WindowKey. Source of truth for "which window owns
    // panel X". Lands in slice A with `windows` so the routing invariant is enforced
    // by code (every panel insert/remove goes through methods that maintain it),
    // not just by tests. See "Message routing" for the rationale.
    pub panel_to_window: HashMap<PanelId, WindowKey>,

    // UNCHANGED — panels stay app-global, layouts only reference IDs:
    pub charts: HashMap<ChartId, ChartPanel>,
    pub watchlists: HashMap<WatchlistId, WatchlistPanel>,
    pub order_panels: HashMap<OrderPanelId, OrderPanel>,
    pub account_panels: BTreeMap<AccountPanelId, AccountPanel>,
    // ... ticker_state, annotation_store, market_cache, router, broker ...
}
```

`is_main` is a flag on `WindowState`, not part of the key. The user can rename "Main" to "Trading" — `main_window_key` follows the rename.

### State-ownership invariants (load-bearing)

All five invariants are upheld by `WindowState` / `MidasApp` methods (no field-level pubs that bypass them):

```
WindowKey  ──1:1──>  iced::window::Id   (via `windows[k].iced_id` and `iced_id_to_key`)
window::Id ──pending──>  WindowKey      (via `pending_window_opens`, drained on WindowAttached)
WindowKey  ──owns──>  WorkspaceLayout   (one pane_grid::State per window)
PanelId    ──1:1──>  WindowKey         (via `panel_to_window`)
PanelId    ──1:1──>  panel data        (HashMap in MidasApp.charts / .watchlists / ...)
```

A panel-keyed handler that finds no entry in `panel_to_window` `tracing::error!`s with the panel ID and drops the message; debug builds additionally `assert!`. Silent drop is reserved for genuinely transient events (close races, in-flight messages from a window that was torn down). A missing entry that surfaces in production indicates upstream state corruption — the metric and log line are how it's detected; we don't paper over it with a no-op alone.

### Message routing — Option B with one carve-out

Two routing models were considered:

- **Option A** (the iced 0.14 `multi_window` example pattern): qualify every per-window `Message` variant with `WindowKey`/`window::Id`, making the (window, payload) coupling type-level. Cost: ~80 variants gain a field, plus closure-construction churn at every `view` build site.
- **Option B** (chosen): qualify only the ~6 pane-grid variants whose payload is intrinsically window-scoped (a `pane_grid::Pane` handle is meaningful only inside one `pane_grid::State`). The other ~80 variants stay flat because they already carry globally-unique panel IDs. The "PanelId → WindowKey" mapping is the runtime invariant `panel_to_window` enforces.

Option B is acceptable only because `panel_to_window` is structurally enforced by helper methods (slice A1) — without that, it would be reckless debt.

~80 of the existing ~100 `Message` variants carry a panel ID (`ChartId`, `WatchlistId`, etc.). Those IDs are already globally unique, so handlers don't need a `WindowKey` to dispatch — they just look up the panel in the global map.

The only variants that **must** be qualified by `WindowKey` are pane-grid messages whose payload is an `iced::widget::pane_grid::Pane` handle (which is scoped to a specific `pane_grid::State`):

```rust
// app.rs — these 5 variants gain WindowKey
PaneFocused(WindowKey, pane_grid::Pane),
PaneClicked(WindowKey, pane_grid::Pane),
PaneResized(WindowKey, pane_grid::ResizeEvent),
PaneDragged(WindowKey, pane_grid::DragEvent),
PaneSplit(WindowKey, pane_grid::Axis, pane_grid::Pane),
PaneClose(WindowKey, pane_grid::Pane),
```

The closure that builds them inside `view_pane_grid(window_key, ws)` already has `window_key` in scope. Cheap `Arc<str>` clone.

**`AddChart` / `AddWatchlist` / `AddOrderPanel` / `AddAccountPanel`** stay payload-less; they read `self.focused_window.clone().unwrap_or_else(|| self.main_window_key.clone())` and target that window's layout.

**Why panel_to_window in slice A.** The official iced 0.14 multi_window example tags every per-window message with `window::Id`, qualifying the (window, payload) coupling at the type level. Option B keeps ~80 panel-data variants un-qualified by relying on the runtime invariant "every PanelId belongs to exactly one window". That invariant is fragile if it lives only in tests — a future code path that creates a panel and forgets to update `panel_to_window` becomes a silent cross-window data corruption bug. Landing `panel_to_window` in slice A and gating panel insertion/removal exclusively through methods that maintain it makes the invariant load-bearing in code, not in tests. This converts what would otherwise be reckless debt into prudent debt with structural backing.

### `view(window_id)` dispatch

```rust
fn view(&self, window_id: window::Id) -> Element<'_, Message> {
    let Some(key) = self.iced_id_to_key.get(&window_id).cloned() else {
        return placeholder_view("(window closing)");
    };
    // .get(), not indexing — window may have been removed by a close handler
    // between the iced_id_to_key lookup and this read in the same view call.
    let Some(ws) = self.windows.get(&key) else {
        return placeholder_view("(window closing)");
    };
    let header = self.view_window_header(&key, ws);
    let body = self.view_pane_grid(&key, ws);
    if ws.is_main {
        column![self.view_toolbar(), header, body, self.view_status_bar()].into()
    } else {
        column![header, body].into()
    }
}
```

### Window titles via `daemon().title(...)`

iced 0.14 has no `iced::window::change_title` API. Per-window titles come from the daemon builder's `.title(...)` chain method, which accepts either a `&'static str` or a `Fn(&State, window::Id) -> String` callback. Today's `main.rs:77-104` already calls `iced::daemon(MidasApp::new, update, view).title("Hand of Midas")`. Replace the static title with the per-window callback:

```rust
// main.rs (existing builder, replacing only the .title argument)
iced::daemon(MidasApp::new, MidasApp::update, MidasApp::view)
    .title(|app: &MidasApp, id: window::Id| match app.iced_id_to_key.get(&id) {
        Some(key) => format!("Hand of Midas — {}", key.as_str()),
        None => "Hand of Midas".to_string(),
    })
    // ... existing settings (subscription, theme, etc.) unchanged
```

Renaming a window mutates the `BTreeMap` key; the next view tick sees the new key and the OS title updates. No imperative `change_title` calls anywhere in the plan. (If iced 0.14's runtime turns out to re-poll `title` only on window events rather than every redraw — the plan does not assume one or the other — `RenameWindow` can issue a no-op `iced::window::resize` or `iced::window::move_to` on the affected id as a redraw nudge. Verify the cadence in the slice C spike.)

Per-window header strip:
- Left: window name as inline-editable text (double-click to rename).
- Right: an `Add ▾` pick-list (`Chart` / `Watchlist` / `Order` / `Account`) and a `+` New Window button.
- Only the **main** window's outer column adds the broker toolbar + status bar — those are global app chrome, not window chrome.

### Window lifecycle

**Startup.** `iced::daemon` does NOT auto-open a window; today's code calls `window::open` explicitly in `MidasApp::new()` (~`app.rs:1865`). Extend that pattern to iterate `config.windows` (main first, then BTreeMap order):

```rust
for (key, wcfg) in startup_order(&config) {
    let (id, open_task) = window::open(window::Settings {
        size: iced::Size::new(wcfg.geometry.width as f32, wcfg.geometry.height as f32),
        position: validated_saved_position(&wcfg.geometry),
        ..window::Settings::default()
    });
    let state = WindowState::from_config(key.clone(), wcfg, id);
    app.windows.insert(key.clone(), state);
    app.iced_id_to_key.insert(id, key.clone());
    app.pending_window_opens.insert(id, key.clone());
    let k = key.clone();
    tasks.push(open_task.map(move |id| Message::WindowAttached(k.clone(), id)));
}
```

After all windows insert, walk `config.windows` again and call `restore_layout_tree` per window into its `WorkspaceLayout`.

**`Message::WindowAttached(WindowKey, window::Id)`** is the open-confirmation. Used to (a) drain `pending_window_opens`, (b) for the main window, mirror `MainWindowOpened` so the existing `WindowGeometry` flow keeps working. Window titles update automatically via the `daemon().title(...)` callback — no imperative call needed.

**Watchdog.** A dedicated 1 Hz `iced::time::every(Duration::from_secs(1))` subscription emits `Message::WindowAttachWatchdog`, separate from the existing `Tick` (which already drives the debounced config save and clock). Decoupling avoids reordering or borrow conflicts inside the existing `Tick` handler. The watchdog handler scans `pending_window_opens` for entries older than 5 s and, for each, fires `Message::WindowAttachFailed(WindowKey)`. The failure handler atomically:

1. Removes the entry from `pending_window_opens`.
2. Removes the entry from `iced_id_to_key`.
3. Removes the `WindowState` from `windows`.
4. Removes any `panel_to_window` entries pointing at that key (and drops the panels from `charts`/`watchlists`/etc., since they were never user-visible).
5. Emits a status-bar toast: `"Window 'X' failed to open"`.

Tested in slice C with a synthetic Task that never resolves and with rapid `WindowAttachWatchdog` ticks during an in-flight open to assert no spurious eviction.

**Create.** `Message::CreateWindow { name: Option<String> }`:
- If `name == None`, generate `"Window N"` via `MidasApp::next_default_window_name()` (scans `windows` for the first free `Window N` with `N ≥ 2`, since `Window 1` reads weird next to `Main`).
- Validate name uniqueness against `self.windows` (case-insensitive); on collision, surface a toast and keep the window unsaved.
- Insert `WindowState` with a `WorkspaceLayout` containing exactly one **placeholder pane**, then `window::open` and chain to `WindowAttached`.

**Empty-window representation.** iced 0.14's `pane_grid::State::new(T)` requires an initial pane — there is no zero-pane state. Today's `WorkspaceLayout::close` already refuses to close the last pane (`layout/mod.rs:223`). We preserve that invariant. The "empty" window is represented by a sentinel `PanelContent::Placeholder` variant rendered as a centred "Click + Add Panel" hint:

```rust
// layout/mod.rs (extended)
pub enum PanelContent {
    Chart(ChartId), Watchlist(WatchlistId), Order(OrderPanelId),
    Account(AccountPanelId), OrderBlotter(OrderBlotterId),
    Placeholder,                                    // NEW
}
```

`Placeholder` panes:
- Are *not* counted by the property test "every panel ID is referenced from exactly one window's layout_tree" — the test scopes to the four real panel kinds.
- Convert in place into a real pane on first `Add*` action (the placeholder pane is replaced by a chart/watchlist/etc., not split). One new helper `WorkspaceLayout::seed_first_pane(content)` mutates the placeholder pane's `PanelContent` and `set_focus`.
- Are not persisted in `LayoutNode` (the leaf simply isn't emitted; on reload an empty `layout_tree` synthesises a single `Placeholder` pane).
- Slice E's popout move uses `WorkspaceLayout::close_pane_for_chart` which, when removing the last real pane, replaces it with a `Placeholder` rather than refusing to close.

This avoids `Option<pane_grid::State>` and the call-site cascade that would follow.

**Close.** `iced::window::Event::CloseRequested` → `Message::WindowCloseRequested(window::Id)`:
- If the closing window is main: flush config, then `iced::exit()` (preserves today's behaviour — quit on main close).
- Else: walk the closed window's layout, drop all referenced panels from app-global maps (`charts`, `watchlists`, `order_panels`, `account_panels`); remove the `WindowState` entry; remove the reverse-lookup entries; mark config dirty; `iced::window::close(id)`. Closing a window destroys its panels — config entry goes away too. (Discussed alternative: keep config entry around for "reopen by name" later. Rejected: creates a foot-gun where a closed window silently reappears next launch with no UI to reach it. Workspace presets are a future feature, not part of this slice.)

**Rename.** `Message::RenameWindow(WindowKey, String)`:
- Validate uniqueness (case-insensitive). On collision, toast and abort.
- Remove from `windows` under old key, re-insert under new key.
- Patch `iced_id_to_key` and `panel_to_window` entries that pointed at the old key.
- If the old key was `main_window_key`, update that cache.
- Mark config dirty. The OS title updates next frame via the `daemon().title(...)` callback.

### Add-panel UX (per window)

Hook `iced::window::Event::Focused`/`Unfocused` in the existing `window_events_sub` (main.rs ~168) to update `focused_window`:

```rust
let window_events_sub = window::events().map(|(id, ev)| match ev {
    iced::window::Event::Focused        => Message::WindowFocused(id),
    iced::window::Event::Unfocused      => Message::WindowUnfocused(id),
    iced::window::Event::Moved(p)       => Message::Window(id, WindowGeometryMsg::Moved(p)),
    iced::window::Event::Resized(s)     => Message::Window(id, WindowGeometryMsg::Resized(s)),
    iced::window::Event::CloseRequested => Message::WindowCloseRequested(id),
    _ => Message::Tick,
});
```

`Unfocused` only clears `focused_window` if the cleared id matches; otherwise it's a stale event from a different window. This avoids racing focus with the OS event order.

Hotkeys (`Ctrl+N` for AddChart, etc., main.rs ~143-151) operate on `focused_window` falling back to `main_window_key` — never a silent no-op.

---

## Config schema (AppConfig v3)

`midas-core/src/config/mod.rs`. Bump `CURRENT_CONFIG_VERSION = 3`.

```rust
pub struct AppConfig {
    pub version: u32,
    pub windows: BTreeMap<String, WindowConfig>,   // NEW
    // unchanged top-level pools (referenced by index from layout_tree leaves):
    pub charts: Vec<ChartConfig>,
    pub watchlists: Vec<WatchlistConfig>,
    pub order_panels: Vec<OrderPanelConfig>,
    pub account_panels: Vec<AccountPanelConfig>,
    pub recent_symbols: Vec<String>,
    pub theme: ThemeConfig,
    pub store: StoreConfig,
    pub providers: Option<ProviderConfig>,
    pub broker: BrokerConnectionConfig,
    pub chart_view_store_schema: u32,
    #[serde(default)] pub levels: HashMap<String, Vec<LevelConfig>>,

    // Legacy fields kept around as #[serde(default, rename = "...")] until v4
    // for one-shot migration, matching the existing `order_blotters` precedent.
    #[serde(default, rename = "window", skip_serializing_if = "Option::is_none")]
    pub legacy_window: Option<WindowGeometryConfig>,
    #[serde(default, rename = "layout_tree", skip_serializing_if = "Vec::is_empty")]
    pub legacy_layout_tree: Vec<LayoutNode>,
    #[serde(default, rename = "panel_order", skip_serializing_if = "Vec::is_empty")]
    pub legacy_panel_order: Vec<PanelSlot>,
}

pub struct WindowConfig {
    #[serde(default)] pub is_main: bool,
    pub geometry: WindowGeometryConfig,
    #[serde(default)] pub layout_tree: Vec<LayoutNode>,
}

// Renamed from the old top-level `WindowConfig` to disambiguate.
pub struct WindowGeometryConfig {
    pub width: u32,
    pub height: u32,
    #[serde(default)] pub maximized: bool,
    #[serde(default)] pub x: Option<i32>,
    #[serde(default)] pub y: Option<i32>,
    #[serde(default)] pub monitor_width: Option<u32>,
    #[serde(default)] pub monitor_height: Option<u32>,
}
```

`ChartConfig` gains an explicit `#[serde(default)] pub id: u32`. **`LayoutNode::Chart` switches from `chart_index: usize` to `chart_id: u32`** in v3 — references survive panel-pool compaction or reordering. Migration assigns `id = position_in_vec` for every existing chart and rewrites every `LayoutNode::Chart { chart_index }` leaf to `LayoutNode::Chart { chart_id: charts[chart_index].id }` in the same pass. Same treatment for `Watchlist`, `OrderPanel`, `Account` leaves and their respective config types.

### v2 → v3 migration

```rust
// migrations.rs
fn migrate_v2_to_v3(cfg: &mut AppConfig) {
    // 1. Assign ids to every panel-pool entry so layout leaves can reference them.
    for (idx, c) in cfg.charts.iter_mut().enumerate() { if c.id == 0 { c.id = idx as u32; } }
    // ... watchlists, order_panels, account_panels ...

    // 2. Rewrite layout_tree leaves from index- to id-based references.
    let layout_tree = rewrite_layout_indices_to_ids(
        std::mem::take(&mut cfg.legacy_layout_tree),
        &cfg.charts, &cfg.watchlists, &cfg.order_panels, &cfg.account_panels,
    );

    let geometry = cfg.legacy_window.take().unwrap_or_default();
    let layout_tree = if layout_tree.is_empty() {
        synthesize_layout_from_panel_order(
            std::mem::take(&mut cfg.legacy_panel_order),
            &cfg.charts, &cfg.watchlists, &cfg.order_panels, &cfg.account_panels,
        )
    } else { layout_tree };

    cfg.windows.insert(
        WindowKey::MAIN_DEFAULT.to_string(),
        WindowConfig { is_main: true, geometry, layout_tree },
    );
    cfg.version = 3;
}
```

**On-disk backup before migration.** Before the first v3 save overwrites `config.toml`, copy the existing v2 file to `config.toml.v2.bak` (one-shot — only written if the backup doesn't already exist). Older binaries refuse to open `version: 3` configs and instead surface a status message pointing the user at the `.v2.bak` for downgrade. This protects against (a) a v3 migration bug corrupting state and (b) accidental cross-version operation.

**Validation pass on load** (idempotent, runs after migration):
- Count `is_main: true`. If 0 → promote first by `BTreeMap` order. If >1 → keep first, demote rest, log `tracing::warn!`.
- If `windows` is empty → synthesize a default `"Main"` entry with default geometry and empty layout.
- Drop layout_tree references to chart ids that don't exist in the panel pool (defensive against hand-edits).

**Forward compat:** `LayoutNode::Unknown` already exists. New `WindowConfig` fields use `#[serde(default)]`. Unknown TOML keys are silently ignored by serde — fine.

---

## UX details

### Per-window header strip (28-32 px)

Layout:

```
┌───────────────────────────────────────────────────────────────────┐
│  Trading                                       [Add ▾]  [+ Window]│
├───────────────────────────────────────────────────────────────────┤
│  pane grid                                                        │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
```

Main window only: above the header strip, the existing toolbar (broker connection, ticker bar, providers) stays. Status bar (bottom) stays in main only too — connection state is global.

`Add ▾` opens a `pick_list` of `Chart`, `Watchlist`, `Order`, `Account`. Selection emits the matching `Add*` message — handlers target `focused_window`.

`+ Window` emits `Message::CreateWindow { name: None }`.

Window-name area is an inline `text_input`-on-double-click. Enter commits → `Message::RenameWindow`. Escape cancels.

Right-click context menu on the header: `Rename…`, `Close window` (greyed if last window, or if main and only main).

### Popout retirement

The chart-panel header's existing pop-out icon now emits `Message::OpenChartInNewWindow(ChartId)`:

```rust
fn handle_open_chart_in_new_window(&mut self, chart_id: ChartId) -> Task<Message> {
    let src_key = self.window_owning_chart(chart_id)
        .unwrap_or_else(|| self.main_window_key.clone());
    let symbol = self.charts.get(&chart_id).map(|c| c.symbol.clone()).unwrap_or_default();
    let name = (!symbol.is_empty()).then(|| format!("{symbol} Chart"));
    if let Some(src) = self.windows.get_mut(&src_key) {
        src.layout.close_pane_for_chart(chart_id);  // new helper, idempotent
    }
    self.create_window_with_seed(name, PaneState::chart(chart_id))
}
```

Move semantics (preserves today's UX). Source pane's `WorkspaceLayout` may end up empty; that's fine because empty layouts are now legal and show a placeholder.

`ChartHandle::Floating(window::Id)` collapses to just `ChartId` — `ChartHandle` retires entirely once slice F1 lands. `Message::PopOut(ChartId)` is **renamed** to `OpenChartInNewWindow(ChartId)` in slice E in the same commit (no `#[deprecated]` alias — it would fire `-D warnings`). Atomic rename across the ~7 call sites.

---

## Window geometry persistence

Today `window_geometry/mod.rs` owns a single `Option<window::Id>`. Generalise: each `WindowState` owns its own `WindowGeometry` instance. The event subscription needs to retain the `id`:

```rust
// main.rs (replacing the existing .map that drops the id)
iced::window::Event::Moved(p)   => Message::Window(id, WindowGeometryMsg::Moved(p)),
iced::window::Event::Resized(s) => Message::Window(id, WindowGeometryMsg::Resized(s)),
```

`Message::Window(window::Id, WindowGeometryMsg)` looks up `iced_id_to_key`, then routes the event into `windows[key].geometry`. Events for unknown ids are dropped silently (handles the close race).

Restore-on-startup: each window's geometry is validated against the current monitor layout. If a window's saved monitor is gone, fall back to default position (centre on primary). One status-bar toast: `"3 windows repositioned: monitor layout changed."`

---

## Dev harness extensions

`desktop/win/crates/midas-devloop-proto/src/lib.rs`:

```rust
// Existing:
Screenshot { out_path: PathBuf }
// becomes (back-compat: window=None means main):
Screenshot { out_path: PathBuf, #[serde(default)] window: Option<String> },

// New variants:
OpenWindow { name: Option<String> },
CloseWindow { name: String },
RenameWindow { from: String, to: String },
ListWindows,                       // returns window keys, display states, layouts
SetWindowFocus { name: String },

// Optional (slice E+):
MoveChartToWindow { chart_id: u32, window: String },
```

`Click`, `ClickPrice`, `Drag`, `Scroll`, `Key`, `OpenOrdersPanel`, `CycleThumbnail` all gain `#[serde(default)] window: Option<String>` (default = main). Coordinates stay per-window client area.

`DumpState` JSON projection adds:
- `windows: { "<key>": { is_main, layout_tree, geometry, iced_id_present } }` (key is the user-visible name)
- `main_window_key: "<key>"`
- `focused_window: "<key>" | null`

The existing top-level `pane_count` continues to mirror `windows[main].layout` panes for one release of harness compat.

`fixture.rs` gains a translator: pre-v3 fixtures with `floating_charts` materialise as a `windows["Popout-<n>"]` entry containing a single-chart layout.

Smoke scripts under `desktop/win/tools/` (`devloop-orders-journey.sh`, `devloop-smoke.sh`) get a sweep — the screenshot path resolves window keys to ids before capture.

---

## Phased slices

Every commit holds `cargo test --workspace` green and `cargo clippy --workspace -- -D warnings` clean in both workspaces. To meet the clippy invariant, fields are introduced in the slice that first uses them — not earlier. No `#[allow(dead_code)]` scaffolding.

**Dependency graph.** Most slices are sequential; B is fully independent of the runtime refactor; G can land alongside C.

```
   A1 ─► A2 ─► C ─► D ─► E ─► F1 ─► F2
                ▲
                └─► G (parallel with C; harness changes are additive)

   B (touches midas-core/config only; lands before or after any of A1..F1
       independently — no edge to draw)
```

### Slice A1 — Introduce types, swap workspace storage in place
- New files: `midas-core/src/window_key.rs` (WindowKey), `midas-app/src/app/window_state.rs` (WindowState), `midas-app/src/app/panel_ids.rs` (PanelIdAllocator, PanelId enum).
- `MidasApp` adds: `windows: BTreeMap<WindowKey, WindowState>` (one entry, key = "Main"), `iced_id_to_key`, `main_window_key`, `panel_ids`, `panel_to_window`.
- **`MidasApp.workspace` is replaced by an accessor**, not duplicated. `pub(crate) fn workspace(&self) -> &WorkspaceLayout { &self.windows[&self.main_window_key].layout }` and a paired `workspace_mut`. The field itself is deleted; existing `self.workspace` reads are now method calls. No mirror, no dual source of truth — `windows[main].layout` is the only storage, and the accessor preserves the existing call-site syntax for slice-A1 mechanical compatibility.
- Panel ID counters move from `WorkspaceLayout` to `PanelIdAllocator`. The few writer sites (`workspace.next_chart_id += 1` etc.) become `self.panel_ids.next_chart()`.
- Every panel mutation path (insert/remove) goes through helper methods on `MidasApp` (e.g. `insert_chart`, `remove_chart`) that maintain `panel_to_window` in lockstep with `windows[*].layout`. The legacy direct-mutation sites are routed through these helpers as part of A1 — without that, A1 isn't actually establishing the invariant.
- Tests pass without modification (accessor preserves call-site shape; helpers keep mutation semantics identical).
- Pre-A2 audit: enumerate test coverage of pane-grid mutations (split, close, drag, resize, focus). If a path is uncovered, add a test in A1 — A2's mechanical churn is only safe under existing test coverage.

### Slice A2 — Migrate call sites off the `workspace()` accessor
- Update ~47 call sites across `app.rs` (6), `app/handlers.rs` (31), `app/views.rs` (5), `app/persistence.rs` (3), `app/fixture.rs` (1), `dev_harness/dump.rs` (1) — counts verified via grep on `self.workspace` / `app.workspace`. Mechanical churn — the structural design is already proven by A1.
- Replace each call with the explicit form (`self.windows[&self.main_window_key].layout` or `self.windows.get_mut(&self.main_window_key).unwrap().layout`), or keep an inline helper. Either way, the `workspace()` accessor and its mut sibling are removed.
- `floating_charts` and `floating_session_charts` untouched.
- **Files:** above six, plus `layout/mod.rs`, `window_geometry/mod.rs`.

### Slice B — Config schema v3 + migration (independent of A; can land before or after)
- Add `WindowConfig`, `WindowGeometryConfig`, `WindowKey` types in `midas-core/src/config/`.
- Bump `CURRENT_CONFIG_VERSION = 3`. Add `legacy_*` rename-shadow fields with `#[serde(default)]` per the precedent.
- Implement `migrate_v2_to_v3` (id assignment + index→id leaf rewrite) and validation pass. Write the v2 backup file before the first v3 save.
- `LayoutNode::Chart`, `Watchlist`, `OrderPanel`, `Account` leaves switch to `*_id: u32`.
- `app/persistence.rs::build_config` walks `windows` and emits one `WindowConfig` per entry.
- Round-trip tests: real v2 configs from the running app; v2 with `panel_order` only; v2 with `floating_charts` populated; v3 byte-stable round-trip.
- **Files:** `midas-core/src/config/mod.rs`, `midas-core/src/config/migrations.rs`, `app/persistence.rs`.

### Slice C — Multi-window create / close / focus / rename
- New messages: `CreateWindow`, `WindowAttached`, `WindowAttachFailed`, `WindowCloseRequested`, `WindowFocused`, `WindowUnfocused`, `RenameWindow`.
- New fields read for the first time here: `focused_window`, `pending_window_opens`.
- `WorkspaceLayout::empty()` + placeholder view ("Click + Add Panel").
- Per-window header strip in `view_window_header(key, ws)`.
- Daemon-level `title(...)` callback wires up window names.
- Hook OS focus events; track `focused_window`.
- `AddChart`/`AddWatchlist`/`AddOrderPanel`/`AddAccountPanel` honour `focused_window`.
- 5-second open watchdog drains `pending_window_opens` + `iced_id_to_key` + `windows` together on attach failure.
- Tests: open + close roundtrip; rename uniqueness; main-close quits; non-main-close doesn't quit; create→add chart→close; close-before-attach race; never-resolving open Task fires `WindowAttachFailed`.

### Slice D — Pane-grid messages get `WindowKey`
- Five pane-grid variants gain `WindowKey` (`PaneFocused`, `PaneClicked`, `PaneResized`, `PaneDragged`, `PaneSplit`, `PaneClose`).
- `view_pane_grid(key, ws)` builds closures capturing the key.
- Dispatcher arms route through `windows[key].layout`.
- `MidasApp.bracket_context_menu` becomes `HashMap<WindowKey, BracketContextMenuState>` (resolves the multi-window race in risk #4 — `panel_to_window` already lands in A1 to support this).
- Tests: pane focus in window B doesn't disturb window A; resizing splits in two windows independently; right-click bracket in window B while context menu is open in window A — both stay live.

### Slice E — Popout migration
- `Message::OpenChartInNewWindow(ChartId)` introduced.
- `Message::PopOut(ChartId)` is **renamed** to `OpenChartInNewWindow` in the same commit (no `#[deprecated]` alias — it would fire warnings under `-D warnings`). Call-site count is small (~7 verified) so atomic rename is the cleanest path.
- Chart-panel header's pop-out icon repointed.
- Each test that asserted on `floating_charts` rewrites to assert on `windows["AAPL Chart"]` (or whatever generated key applies). Slice E's "done when" includes "no test reads `floating_charts` directly; all read `windows[…]`."
- **Synthetic-ChartId scrub.** `subscription_registry::CHART_REGISTRY` is drained of high-bit synthetic keys when a popout migrates. Add a debug-build `assert!` in slice F that no `CHART_REGISTRY` key has bit 31 set after the deletion lands.

### Slice F1 — Delete `floating_charts`
- Drop `floating_charts: HashMap<window::Id, ChartPanel>` from `MidasApp`.
- Drop `ChartHandle::Floating(window::Id)`. `ChartHandle` retires entirely (or simplifies to `ChartId`).
- **Delete** `floating_window_synthetic_id` (handlers.rs ~4354) — its only producers are gone after slice E, so a stale call site becomes a `cargo build` failure rather than a runtime check. Stronger than a debug `assert!`.
- The slice E debug-build assert ("no `CHART_REGISTRY` key has bit 31 set") stays in place as a belt-and-braces check that no fixture or test reintroduces a synthetic key.
- Verified call-site counts: `floating_charts` references = 40 across `app.rs` (16), `handlers.rs` (23), `views.rs` (1); `ChartHandle` references = 32 across the same three files. Once E rewires the popout entry path, F1 is genuinely a removal diff.

### Slice F2 — Retire `session_chart_window.rs` (gated spike)
- Pre-work (timeboxed 1 day): build a stub `PaneState::SessionChart(SessionChartId)` rendering a minimal `iced::widget::shader` clear-colour inside a pane cell. Verify cursor capture, focus, and redraw scheduling vs. a standalone window. **Gate 1**: if any of these don't work in iced 0.14's `pane_grid::Pane` cells, F2 is no-go.
- Port (timeboxed 2-3 days): fold the 345 LOC from `session_chart_window.rs` into the `PaneState::SessionChart` rendering path. Wire up the existing driver task lifetime and watch receivers. **Gate 2**: if porting blows past 3 days, F2 is no-go.
- Total budget 3-4 days with two go/no-go gates. If either fails, F2 is deferred indefinitely; slice F1 still ships, and `floating_session_charts` remains feature-gated on `session_chart` until a follow-up effort. The plan does not block on F2 — F1 alone clears the legacy popout for the default (non-`session_chart`) build.

### Slice G — Dev harness extensions (parallelisable with C onward)
- Schedule note: G can land alongside C. The harness commands (`OpenWindow`, `CloseWindow`, `RenameWindow`, `ListWindows`) actively help test C; running them in lockstep is faster than back-loading.
- `midas-devloop-proto` updates: `Screenshot { window: Option<String> }`, new commands `OpenWindow`, `CloseWindow`, `RenameWindow`, `ListWindows`, `SetWindowFocus`. `Click`/`ClickPrice`/`Drag`/`Scroll`/`Key`/`OpenOrdersPanel`/`CycleThumbnail` gain optional `window: Option<String>` (default = main).
- `DumpState` projection update.
- Smoke scripts swept for window-aware fixture journeys.
- New devloop journey: `multi-window-journey.sh` opens two windows, adds chart in each, screenshots both.

---

## Verification

### Unit / integration tests
- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` clean at every slice boundary in both root and `desktop/win`.
- Round-trip tests for v2 → v3 config migration, including:
  - Real config files captured from the running app today.
  - Fixtures with `floating_charts` populated.
  - Configs with `panel_order` fallback and no `layout_tree`.
- Property test: name uniqueness round-trips through TOML save/load.
- Property test: every panel ID is referenced from exactly one window's layout_tree (no orphans, no double-references).

### Dev harness journey (slice C and after)
1. Boot fresh: `cargo run -p midas-app --features dev_harness`.
2. Drive: `OpenWindow { name: "Scanner" }` → `WaitForIdle` → `Screenshot { window: Some("Scanner") }`.
3. `Click` (with `window: Some("Scanner")`) the `+ Add Panel` placeholder → expect a chart pane.
4. `RenameWindow { from: "Scanner", to: "Day Trading" }` → screenshot, expect title bar change.
5. `Shutdown`. Reload from disk: confirm `windows["Day Trading"]` matches the layout we built.
6. Add a multi-monitor variant once `WindowGeometry` validation lands.

### Manual smoke
- Cold-start the app on a fresh user profile → only the default Main window appears.
- Add a chart, open a new window, add a watchlist to the new window, rename the new window to "Levels", close it. Reopen the app → both windows restored with their contents and positions.
- Disconnect the second monitor before launching with windows that had been on it → those windows recenter on primary; toast appears.
- Right-click the chart pop-out icon → window opens with that chart only; source pane is replaced (if it was the only chart there, source window becomes empty placeholder).
- Press `Ctrl+N` while window B is focused → chart added in window B, not main.

---

## Critical files

| Area | Path |
|---|---|
| Top-level app + Message + dispatch | `desktop/win/crates/midas-app/src/app.rs` |
| Handlers | `desktop/win/crates/midas-app/src/app/handlers.rs` |
| Views | `desktop/win/crates/midas-app/src/app/views.rs` |
| Persistence builder | `desktop/win/crates/midas-app/src/app/persistence.rs` |
| Layout (pane grid) | `desktop/win/crates/midas-app/src/layout/mod.rs` |
| Window geometry | `desktop/win/crates/midas-app/src/window_geometry/mod.rs` |
| New: WindowState | `desktop/win/crates/midas-app/src/app/window_state.rs` |
| New: PanelIdAllocator | `desktop/win/crates/midas-app/src/app/panel_ids.rs` |
| Config schema | `desktop/win/crates/midas-core/src/config/mod.rs` |
| Config migrations | `desktop/win/crates/midas-core/src/config/migrations.rs` |
| New: WindowKey | `desktop/win/crates/midas-core/src/window_key.rs` |
| Devloop protocol | `desktop/win/crates/midas-devloop-proto/src/lib.rs` |
| Devloop fixture | `desktop/win/crates/midas-app/src/app/fixture.rs` |
| Devloop screenshot | `desktop/win/crates/midas-app/src/dev_harness/screenshot.rs` |
| Devloop dump | `desktop/win/crates/midas-app/src/dev_harness/dump.rs` |
| Smoke scripts | `desktop/win/tools/devloop-*.sh` |
| Main entry / event subscription | `desktop/win/crates/midas-app/src/main.rs` |

---

## Risks / open questions

1. **Hotkey vs. focus race.** `Ctrl+N` fires from a global `keyboard::listen` subscription regardless of which window has OS focus. The mitigation (target `focused_window`, fall back to main) avoids silent failure but can target the "wrong" window during a focus transition. The harness should exercise rapid alt-tab + Ctrl+N to confirm this isn't user-visible.
2. **Iced cross-window pane drag.** `PaneGrid::on_drag` doesn't support cross-window drop. Out of scope (see Non-Goals). UX answer: use the pop-out icon. `MoveChartToWindow` lives in dev harness; user-facing equivalent is a follow-up if requested.
3. **`window::open` failure path.** `WindowAttachFailed(WindowKey)` + 5-second watchdog drains `pending_window_opens`, `iced_id_to_key`, and `windows` atomically (specified in lifecycle section, tested in slice C).
4. **Bracket context menu race.** `MidasApp.bracket_context_menu` becomes `HashMap<WindowKey, BracketContextMenuState>` in slice D, supported by `panel_to_window` from slice A1.
5. **Panel orphan cleanup on window close.** Closing a window destroys its panels. Documented; tests cover it. "Save and reopen later" is in the workspace-presets follow-up, not this slice.
6. **Subscription identity for retired floating charts.** `subscription_registry` keys by a synthetic chart ID derived from `window::Id` (handlers.rs ~4354 — high bit set on hash of `window::Id`). Slice E drains synthetic keys as each popout migrates. Slice F1 adds a debug-build assert that no `CHART_REGISTRY` key has bit 31 set after deletion. `PanelIdAllocator` mints linear u32 from a low base; in extreme sessions it could approach the high-bit reserved range — flag for monitoring, not a slice blocker.
7. **Session-chart retirement.** Slice F2 is gated on a 2-day spike to integrate `iced::widget::shader` with `pane_grid` cells. Go/no-go decision at end of spike. If no-go, F1 ships alone; `floating_session_charts` (feature-gated on `session_chart`) stays until a follow-up.
8. **Monitor restore.** Per-window geometry validation falls back to primary on missing monitor. One status-bar toast aggregates the count.
9. **Window-name normalisation.** Trim, max 64 chars, case-insensitive uniqueness, display preserves user's case. Forbid empty / whitespace-only. Validate at `WindowKey::normalize`.
10. **First-launch UX.** Fresh install opens one `Main` window. Discoverability of "+ Window" — consider a one-time tooltip or empty-state hint in a follow-up if usage shows users don't find it.
11. **Test coverage of pane-grid mutations before slice A1's storage swap.** `app.rs`/`handlers.rs` are large (4,778 + 5,003 lines). Before slice A1 deletes the `MidasApp.workspace` field and routes reads through the accessor, audit existing tests for pane-grid mutation coverage (split/close/drag/resize/focus). If coverage is thin in any path, add tests *first* — that's the safety net for the routing-helper migration in A1 and the call-site churn in A2.
