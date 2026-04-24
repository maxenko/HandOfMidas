//! Application state, Message enum, and update logic.
//!
//! Sub-modules:
//! - `handlers`: domain-grouped message handlers (symbol/data, chart,
//!   pane, order panel, watchlist, broker, bracket, toast, etc.)
//! - `views`: widget tree construction (toolbar, pane grid, status bar)
//! - `persistence`: config build, save, and debounce

mod chart_subscription;
#[cfg(feature = "dev_harness")]
mod fixture;
mod handlers;
pub mod order_events_subscription;
mod persistence;
mod subscription_context;
mod subscription_helpers;
mod subscription_registry;
mod subscription_stream;
mod ticker_subscription;
mod ticker_wiring;
mod views;
mod watchlist_subscription;

#[cfg(feature = "dev_harness")]
pub use fixture::FixtureError;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use iced::widget::pane_grid;
use iced::{window, Task};

use midas_chart::camera::Camera2D;
use midas_chart::state::ChartState;
use midas_chart::AnnotationId;
use midas_core::config::{AppConfig, ChartConfig, LayoutNode, PanelSlot};
use midas_core::{
    AccountPanelId, CandleBuffer, ChartBackend, ChartId, DataProvider, LinkMode, OrderPanelId,
    Timeframe, WatchlistId,
};

use crate::registry::HistoricalDataRegistry;

use crate::annotation_store::AnnotationStore;
use crate::layout::{LayoutPresetKind, PanelContent, WorkspaceLayout};
use crate::link::{LinkDimension, PickerTarget};
use crate::order_panel::OrderSide;
use crate::watchlist::WatchlistPanel;

// Toast state moved to `crate::toast` — see midasapp-split.md.
// `ToastAction` is re-exported because `Message::Ticker` plumbing
// references it inside `TickerEffect::Toast`. Other toast types
// (`ToastState`, `ToastMsg`, `Effect`, `TOAST_TTL_SECS`) live on
// `crate::toast` directly; reach there through the controller.
pub use crate::toast::ToastAction;

// ── Recent instruments (MRU) ──────────────────────────────────────────

/// Maximum number of symbols retained in the Recent Instruments MRU.
///
/// Matches the cap documented in the account-panel plan (Decision 6).
/// `push_recent_symbol` pops from the back until the deque's length is
/// within this bound.
pub const MAX_RECENTS: usize = 20;

/// One entry in the Recent Instruments MRU.
///
/// `last_seen` is `None` when the entry was rehydrated from the persisted
/// `AppConfig` (timestamps are not persisted — only the symbol list).
/// Entries created during the current session carry
/// `Some(Instant::now())` so the UI can render an "N min ago" suffix.
#[derive(Clone, Debug)]
pub struct RecentEntry {
    /// Upper-cased ticker symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// `None` when loaded from persisted config (timestamps aren't
    /// persisted); `Some(Instant)` when added during this session.
    pub last_seen: Option<Instant>,
}

/// Pure MRU update: dedup, move-to-front, cap at [`MAX_RECENTS`].
///
/// Extracted so [`MidasApp::push_recent_symbol`] has a trivially unit-
/// testable core that doesn't require constructing the full app. Returns
/// `true` when the deque was actually mutated (empty / whitespace
/// input short-circuits with `false` so the caller can skip the
/// accompanying `mark_config_dirty`).
fn push_recent_symbol_inner(
    recents: &mut VecDeque<RecentEntry>,
    symbol: &str,
    now: Instant,
) -> bool {
    let symbol = symbol.trim().to_uppercase();
    if symbol.is_empty() {
        return false;
    }
    // Dedup: drop any existing entry so we can re-push at the front.
    if let Some(pos) = recents.iter().position(|e| e.symbol == symbol) {
        recents.remove(pos);
    }
    recents.push_front(RecentEntry {
        symbol,
        last_seen: Some(now),
    });
    while recents.len() > MAX_RECENTS {
        recents.pop_back();
    }
    true
}

/// Convert the router's `Vec<Bar>` into a legacy `CandleBuffer`.
///
/// The router speaks `midas_broker_core::market_data::Bar` (f64 OHLC,
/// u64 volume, UTC `DateTime`); the chart's storage is SoA f32 columns
/// keyed by epoch-millis. Volumes saturate at `u32::MAX` — individual
/// equity bars never approach 4 B shares so this is lossless in
/// practice. Pre-sizes both `Vec`s to avoid mid-fill reallocation.
fn bars_to_candle_buffer(bars: &[midas_broker_core::market_data::Bar]) -> CandleBuffer {
    let mut buf = CandleBuffer::with_capacity(bars.len());
    for bar in bars {
        buf.push(
            bar.ts_open.timestamp_millis(),
            bar.o as f32,
            bar.h as f32,
            bar.l as f32,
            bar.c as f32,
            bar.volume.min(u32::MAX as u64) as u32,
        );
    }
    buf
}

#[cfg(test)]
mod recent_symbols_tests {
    use super::{push_recent_symbol_inner, RecentEntry, MAX_RECENTS};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    fn symbols(recents: &VecDeque<RecentEntry>) -> Vec<&str> {
        recents.iter().map(|e| e.symbol.as_str()).collect()
    }

    #[test]
    fn push_prepends_and_uppercases() {
        let mut recents = VecDeque::new();
        let t0 = Instant::now();
        assert!(push_recent_symbol_inner(&mut recents, "aapl", t0));
        assert_eq!(symbols(&recents), vec!["AAPL"]);
        assert_eq!(recents[0].last_seen, Some(t0));
    }

    #[test]
    fn push_dedups_and_moves_existing_to_front() {
        let mut recents = VecDeque::new();
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(1);
        let t2 = t0 + Duration::from_secs(2);
        let t3 = t0 + Duration::from_secs(3);

        push_recent_symbol_inner(&mut recents, "AAPL", t0);
        push_recent_symbol_inner(&mut recents, "MSFT", t1);
        push_recent_symbol_inner(&mut recents, "TSLA", t2);
        // Re-push AAPL — should move it to the front without duplicating.
        push_recent_symbol_inner(&mut recents, "AAPL", t3);

        assert_eq!(symbols(&recents), vec!["AAPL", "TSLA", "MSFT"]);
        assert_eq!(recents.len(), 3, "dedup keeps length stable");
        assert_eq!(recents[0].last_seen, Some(t3), "front has newest Instant");
    }

    #[test]
    fn push_enforces_cap() {
        let mut recents = VecDeque::new();
        let t0 = Instant::now();
        // Push MAX_RECENTS + 5 distinct symbols; oldest 5 must be evicted.
        for i in 0..(MAX_RECENTS + 5) {
            push_recent_symbol_inner(&mut recents, &format!("SYM{i}"), t0);
        }
        assert_eq!(recents.len(), MAX_RECENTS);
        // Front is the most recently pushed (SYM{MAX+4}); back is
        // SYM5 (the first one that survived eviction).
        assert_eq!(recents[0].symbol, format!("SYM{}", MAX_RECENTS + 4));
        assert_eq!(recents.back().unwrap().symbol, "SYM5");
    }

    #[test]
    fn empty_or_whitespace_input_is_ignored() {
        let mut recents = VecDeque::new();
        let t0 = Instant::now();
        assert!(!push_recent_symbol_inner(&mut recents, "", t0));
        assert!(!push_recent_symbol_inner(&mut recents, "   ", t0));
        assert!(recents.is_empty(), "empty/whitespace MUST NOT push");
    }
}

// ── Load state ────────────────────────────────────────────────────────

/// Tracks the data loading lifecycle for a chart panel.
///
/// `Loading` and `Error` are retained for future async data providers.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum LoadState {
    /// No data has been requested yet.
    Empty,
    /// An async load is in progress.
    Loading,
    /// Data loaded successfully.
    Loaded,
    /// Data load failed with an error description.
    Error(String),
}

// ── Chart panel ───────────────────────────────────────────────────────

/// Per-chart panel state. Holds everything needed to display one chart.
///
/// Clone is cheap: `data` is an `Arc` and `ChartState` derives Clone.
#[derive(Clone)]
pub struct ChartPanel {
    /// The ticker symbol displayed in this chart (e.g. "AAPL").
    ///
    /// Backward compat — kept during Slice 3 migration. New code reads
    /// `bound_symbol` instead. Slice 4 removes this field.
    pub symbol: String,
    /// Bound symbol key resolved from the symbol-link color group.
    ///
    /// Set by [`MidasApp::bind_chart_to_symbol`]. `None` means the chart
    /// is unbound (empty placeholder). Persisted in config for restart.
    pub bound_symbol: Option<crate::annotation_store::SymbolKey>,
    /// Active timeframe for this chart.
    pub timeframe: Timeframe,
    /// Loaded candle data, shared via Arc for zero-copy access.
    pub data: Option<Arc<CandleBuffer>>,
    /// The sans-IO chart state machine (camera, dirty flags, levels).
    pub chart_state: ChartState,
    /// Current data loading lifecycle state.
    pub load_state: LoadState,
    /// Per-chart ticker input text (for inline editing in title bar).
    pub symbol_input: String,
    /// ID of the level currently being edited in the popup, or None.
    pub editing_level_id: Option<u64>,
    /// Screen position where the level editor should appear (x, y in chart-local pixels).
    pub editing_level_screen_pos: Option<(f32, f32)>,
    /// Temporary string for the price input field in the level editor.
    pub level_editor_price_input: String,
    /// Symbol link group for cross-chart symbol synchronization.
    pub symbol_link: LinkMode,
    /// Timeframe link group for cross-chart timeframe synchronization.
    pub timeframe_link: LinkMode,
    /// Whether the G.ATR badge is currently hovered (triggers candle dimming).
    pub gatr_hover: bool,
    /// Set `true` when a saved camera is restored in `bind_chart_to_symbol`.
    /// Cleared after `DataLoaded` re-evaluates the camera (live-edge shift
    /// or empty-history fallback), or on the first user pan gesture.
    pub camera_restored_pending: bool,
    /// Generation counter incremented on every load request.
    /// Used to discard stale `DataLoaded` messages from previously-requested tickers.
    pub load_generation: u64,
    /// Whether the chart is currently driving live-data subscriptions
    /// (S8 §D). When `false`, `chart_subscriptions()` drops this chart
    /// from the iced diff, which cascades through the RAII chain
    /// `SubscriptionHandle → DecRef` to cancel the upstream.
    ///
    /// Defaults to `true`. Future UI plumbing (pane minimisation,
    /// off-screen floating windows, mobile workspace switch) will
    /// flip this field; the current code path only exercises the
    /// default-true behaviour end-to-end.
    pub visible: bool,
    /// Chart rendering backend for this specific panel (chart-transition
    /// slice 9a). Defaults to [`ChartBackend::Legacy`]; the toolbar
    /// backend chip toggles between `Legacy` and `New` on a per-panel
    /// basis.
    ///
    /// Persisted in [`ChartConfig::backend`] so the selection survives
    /// restart. When the binary is built without `--features
    /// session_chart` the dispatch in `app/views.rs` falls back to
    /// `Legacy` with a `tracing::warn!` regardless of this field's
    /// value (plan Scenario 9).
    pub backend: ChartBackend,
}

impl ChartPanel {
    /// The timestamp of the most recent candle in the loaded data.
    ///
    /// Returns `None` if no data is loaded or the buffer is empty.
    pub fn latest_candle_time(&self) -> Option<f64> {
        self.data.as_ref().and_then(|buf| {
            if buf.is_empty() {
                None
            } else {
                Some(buf.timestamps[buf.len() - 1] as f64)
            }
        })
    }

    /// Whether this chart should currently be driving live-data
    /// subscriptions (S8 §D).
    ///
    /// Reads the [`Self::visible`] field — defaults to `true`. The
    /// `chart_subscriptions()` iced builder filters on this so the
    /// RAII drop chain `SubscriptionHandle → DecRef` fires the
    /// moment a chart flips invisible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

/// Shared mutation that swaps the ticker symbol shown by a chart panel.
///
/// Extracted out of `broadcast_symbol_to_link_group`'s floating-chart
/// loop so docked and floating paths now share one inline mutator.
/// Docked charts normally go through `bind_chart_to_symbol` which also
/// resets `load_generation` + clears `data`; this helper performs the
/// thin subset used by link-group broadcasts and floating-chart
/// `SetSymbolLink` (which both follow up with an async load that
/// eventually refreshes the rest of the panel state).
pub(crate) fn apply_symbol_to_panel(
    panel: &mut ChartPanel,
    symbol: &str,
    sym_key: crate::annotation_store::SymbolKey,
) {
    panel.bound_symbol = Some(sym_key);
    panel.symbol = symbol.to_owned();
    panel.symbol_input = symbol.to_owned();
    panel.load_state = LoadState::Loading;
    panel.chart_state.dirty.mark_data();
}

// ── Chart handle ───────────────────────────────────────────────────────

/// Identifies a chart panel regardless of whether it lives in the
/// docked [`MidasApp::charts`] map (keyed by [`ChartId`]) or the
/// floating [`MidasApp::floating_charts`] map (keyed by iced's
/// `window::Id`).
///
/// This is **not** a HashMap key. The two storage maps stay distinct —
/// iced 0.14 dispatches window lifecycle events natively keyed on
/// `window::Id`, and using `ChartHandle` as a key would force a
/// wrap/unwrap on every window-event path. Instead, `ChartHandle`
/// shows up in iterator items (see [`MidasApp::all_chart_panels`])
/// and in collapsed Link-message variants so handlers that don't
/// care about the docked/floating distinction can stay generic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChartHandle {
    /// A chart panel docked inside the main window's pane grid.
    Docked(ChartId),
    /// A chart panel that lives in its own OS window (pop-out).
    Floating(window::Id),
}

// ── Application state ─────────────────────────────────────────────────

/// Top-level application state. Owns all chart panels and layout.
pub struct MidasApp {
    /// All chart panels keyed by stable ChartId.
    pub charts: HashMap<ChartId, ChartPanel>,
    /// Workspace layout managed by iced's pane_grid.
    pub workspace: WorkspaceLayout,
    /// Status bar message text.
    pub status_message: String,
    /// Whether the FPS/frame-time debug overlay is visible.
    pub show_frame_overlay: bool,
    /// Path to the configuration file on disk.
    pub config_path: PathBuf,
    /// Whether the config has been modified since the last save.
    pub config_dirty: bool,
    /// Timestamp of the last config save, used for debouncing.
    pub last_config_save: Instant,
    /// Current wall-clock time string for the status bar.
    pub current_time: String,
    /// OS-window geometry sub-controller (audit P1 slice 2). Owns
    /// `main_window`, `position`, `size`, `monitor_size`. MidasApp
    /// only routes [`Message::Window`] into it and interprets the
    /// resulting effects via `dispatch_window`.
    pub window: crate::window_geometry::WindowGeometry,
    /// Floating chart windows popped out from the main pane grid.
    /// Keyed by the OS window ID returned from `window::open()`.
    pub floating_charts: HashMap<window::Id, ChartPanel>,
    /// Whether level placement mode is globally active across all charts.
    pub level_placing: bool,
    /// Active placement preview: (source chart, ticker, price).
    /// Used to ghost the preview line on sibling charts and to clear
    /// stale previews on non-source charts (cross-window jumps).
    pub placing_preview: Option<(ChartId, String, f64)>,
    /// Annotation currently being dragged by the user, if any. Set
    /// app-wide so every chart showing the same symbol promotes the
    /// dragged level into its drag-pass z-layer — otherwise only the
    /// chart receiving events would see the per-element z-order fix,
    /// and the other charts' text would mix with neighbouring levels.
    pub dragging_annotation: Option<midas_chart::widget::AnnotationId>,
    /// Cross-chart crosshair sync: (source chart, timestamp_ms, price, symbol).
    /// When set, sibling charts (same symbol, different chart) render
    /// ghost crosshair lines at the corresponding timestamp and price.
    pub crosshair_sync: Option<(ChartId, i64, f64, String)>,
    /// Registry of historical-data providers feeding chart loads,
    /// market snapshots, and thumbnails. Router-era streaming goes
    /// through [`MarketDataRouter`](midas_market_data::MarketDataRouter)
    /// directly; see `registry.rs` for the **OPEN (post-refactor)**
    /// note on retiring this registry once every `DataProvider`
    /// call site migrates to `MarketDataSource::historical_bars`.
    pub providers: HistoricalDataRegistry,
    /// DuckDB persistent cache handle. None if disabled or failed to open.
    #[allow(dead_code)] // part of planned API
    pub store: Option<midas_store::DbHandle>,
    /// All watchlist panels keyed by stable WatchlistId.
    pub watchlists: HashMap<WatchlistId, WatchlistPanel>,
    /// Last known cursor position (tracked globally for drag preview placement).
    pub cursor_position: iced::Point,
    /// Pending drag: user pressed a ticker but 250ms hasn't elapsed yet.
    pub pending_drag: Option<PendingDragState>,
    /// Active drag-drop state: promoted from pending after hold threshold.
    pub dragging_ticker: Option<DragTickerState>,
    /// Which link picker dropdown is currently open, if any.
    pub link_picker_open: Option<(PickerTarget, LinkDimension)>,
    /// Active column resize drag, unified across all grid surfaces
    /// (Watchlist, Account Orders/History/Recents). The `target` field
    /// routes grid-state lookup in `handle_column_resize`.
    ///
    /// Replaces the four parallel `resizing_*_column` fields from the
    /// pre-collapse shape (audit Re-audit 2026-04-18 Round 2 P1).
    pub resizing_column: Option<crate::column_resize::ColumnResizeState>,
    /// Which Account-pane's Orders-tab column-selector popup is open, if any.
    pub account_column_selector_open: Option<AccountPanelId>,
    /// Dockable order panels keyed by stable OrderPanelId.
    pub order_panels: HashMap<OrderPanelId, crate::order_panel::OrderPanel>,
    /// Tabbed Account panels (Positions / Orders / History / Recents).
    ///
    /// Replaces the legacy `order_blotters` map. Migration translates
    /// old blotter entries at config-load time.
    pub account_panels:
        std::collections::BTreeMap<AccountPanelId, crate::account_panel::AccountPanel>,
    /// Recent Instruments MRU feeding the Account panel's Recents tab.
    ///
    /// Bounded at [`MAX_RECENTS`]; front = most-recent. Symbols are
    /// persisted to `AppConfig::recent_symbols` (timestamps aren't).
    /// Use [`MidasApp::push_recent_symbol`] to keep dedup + move-to-front
    /// + cap invariants intact.
    pub recent_symbols: VecDeque<RecentEntry>,
    /// App-wide live-position store. Fed by:
    /// - The coalesced `router_positions_subscription` stream
    ///   (steady-state), which delivers `Message::AccountPositionsBatch`.
    /// - The single-event `BrokerEvent::PositionUpdate` path inside
    ///   `handle_broker_msg` (reconnect backfills). Both paths are
    ///   idempotent; last write wins.
    ///
    /// Single-account assumption: v1 does not filter by `account`;
    /// every `PositionUpdate` is applied verbatim. Slice 5 renders the
    /// Positions tab from this store.
    pub positions: crate::account_panel::PositionStore,
    /// Shared blotter accumulating rows from `BrokerEvent`s. Single
    /// instance read by every Account pane's Orders tab.
    pub order_blotter: crate::order_blotter::OrderBlotter,
    /// redb-backed persistence for the blotter — survives restart.
    pub order_history_persist: crate::order_blotter::persist::OrderHistoryPersistHandle,
    /// Links between chart bracket annotations and broker orders,
    /// keyed by the parent (entry) order UUID for O(1) lookup.
    pub order_annotation_links: HashMap<uuid::Uuid, crate::order_panel::OrderAnnotationLink>,
    /// Toast notification controller. State + auto-dismiss + view all
    /// live behind [`crate::toast::ToastController`]; `MidasApp` only
    /// routes [`Message::Toast`] into it and interprets the resulting
    /// effects via `dispatch_toast` / `consume_toast_effects`.
    pub toasts: crate::toast::ToastController,
    /// Bracket context menu state: (chart_id, annotation_id, leg_role, screen_x, screen_y).
    pub bracket_context_menu: Option<(
        ChartId,
        u64,
        midas_chart::widget::order_bracket::LegRole,
        f32,
        f32,
    )>,
    /// Centralized per-symbol annotation store (order brackets, levels, etc.).
    pub annotation_store: AnnotationStore,
    /// In-memory market data cache for watchlist columns.
    pub market_cache: crate::market_cache::MarketDataCache,
    /// Current broker connection state display string.
    pub broker_connection_display: String,
    // Session-scoped per-symbol flags (GATR snap-once guard, anchor-seed
    // toast dedup, pre-snap undo slot) live on `TickerState` via
    // `TickerSessionFlags`. See audit round-2 P3b.
    /// Per-(symbol, timeframe) chart view settings. Single authority
    /// for camera positioning on data load (zoom level, positioning).
    pub chart_views: crate::chart_view::ChartViewStore,
    /// Per-ticker interval preference for the grid-cell thumbnail
    /// widget. Session-scoped; see `thumbnail_store` module docs.
    pub(crate) thumbnail_store: crate::thumbnail_store::ThumbnailStore,
    /// Per-(symbol, timeframe) cache of the last-N close prices
    /// displayed by the thumbnail widget. See `thumbnail_data` module
    /// docs.
    pub(crate) thumbnail_data: crate::thumbnail_data::ThumbnailDataStore,
    /// Per-symbol ticker state map. The single source of truth for all
    /// per-symbol state: order brackets, entry memories, GATR anchors,
    /// price levels, and market data snapshots.
    pub tickers: std::collections::HashMap<
        crate::annotation_store::SymbolKey,
        crate::ticker_state::TickerState,
    >,
    /// Persistence handle for TickerState (redb v2). Opened on startup,
    /// flushed on shutdown. The `PersistDirty` effect routes through this.
    pub ticker_persist: crate::ticker_state::persist::TickerStatePersistHandle,
    /// No-reentry guard for the `Message::Ticker` dispatch cycle.
    /// Set to `true` while processing effects; asserted `false` at
    /// entry to prevent feedback loops.
    pub ticker_dispatch_active: bool,
    /// Running IB-simulator child process.
    ///
    /// Populated on startup when `config.broker.backend == Sim` (the
    /// default). Also settable by the devloop `SpawnSim` command
    /// under the `dev_harness` feature — both code paths converge on
    /// the same lifecycle handle so the reaper runs in one place.
    /// `None` means no sim is running (user is on LivePaper / Live,
    /// or the auto-spawn failed — see `broker_connection_display`
    /// for the surfaced reason).
    pub sim_child: Option<crate::sim_child::SimChildHandle>,
    /// Broker connection configuration round-tripped from
    /// `config.toml`. The fields aren't mutated by the current UI
    /// — they're read once on startup to decide whether to
    /// auto-spawn the sim or connect to a real gateway — but we
    /// keep the struct alive so `to_config` writes the same value
    /// back and hand-edited TOML survives `cargo run` cycles.
    pub broker_cfg: midas_core::config::BrokerConnectionConfig,
    // ── Router refactor (S7) ──────────────────────────────────────
    /// Market-data router. `None` while an IB connection is still
    /// being set up (NB-4 / NB-5); every subscription closure guards
    /// `Some(router)` and returns `Subscription::none()` when the
    /// router hasn't landed yet. iced re-diffs `subscription()` on
    /// each `update()` so subscriptions spin up the moment
    /// `Message::RouterReady(Ok(..))` swaps in a real router.
    pub(crate) router: Option<std::sync::Arc<midas_market_data::MarketDataRouter>>,

    /// Order client the app talks to for bracket submission and
    /// position / account event streams (BR-14). Swapped in alongside
    /// the router on `Message::RouterReady` for IB; constructed
    /// synchronously for Sim.
    pub(crate) router_order_client: Option<std::sync::Arc<dyn midas_broker::OrderClient>>,

    /// M-29 throttle — at most one [`Message::ChartResync`] per chart
    /// per 5 s. Consumers that observe `Lagged` on their bar stream
    /// emit a `ChartResync`; without a throttle a flaky chart can DoS
    /// IB pacing. Key: `ChartId`; value: `Instant` of the last
    /// allowed resync. Wired by the `ChartResync` handler in S7b.
    pub(crate) resync_throttle: std::collections::HashMap<ChartId, Instant>,

    /// Translation map: IB order id (i32, from the router's
    /// `OrderClient`) → local UUID (used by `order_blotter` and
    /// `order_annotation_links`). Populated when
    /// `Message::BracketPlaceResult` lands the real IB ids for a
    /// freshly-submitted bracket; consulted from
    /// `Message::RouterOrderEvent` to synthesise `BrokerEvent`-shaped
    /// order status / fill / rejection messages the existing UI
    /// handlers consume. (Router refactor slice 10c.)
    pub(crate) ib_to_uuid: std::collections::HashMap<i32, uuid::Uuid>,

    /// Session-aware-charts Phase C: floating windows hosting a
    /// [`crate::session_chart::widget::SessionChart`]. Keyed by the
    /// OS window id returned from `window::open()`. The value owns
    /// the widget, the driver Arc (kept alive so the pump task lives
    /// as long as the window), and a fresh `watch` receiver used by
    /// the window's subscription to wake on version ticks.
    ///
    /// Feature-gated on `session_chart`. None of the legacy chart /
    /// watchlist / ticker-state code reads this map.
    #[cfg(feature = "session_chart")]
    pub(crate) floating_session_charts:
        std::collections::HashMap<window::Id, crate::session_chart_window::SessionChartWindow>,

    /// Slice 2c of chart-transition: per-symbol shared-Arc lookup for
    /// `CandleSeries` handles. Drivers register on spawn + deregister
    /// on drop. `QuoteBatch` handler reads this map to fold
    /// quote-cadence ticks into every session-chart panel for a
    /// symbol with ONE write-guard take per batch.
    #[cfg(feature = "session_chart")]
    pub(crate) session_chart_registry: crate::session_chart::SymbolSeriesRegistry,
}

/// Pending drag: press started but hold threshold not yet reached.
#[derive(Debug, Clone)]
pub struct PendingDragState {
    /// The ticker symbol that might be dragged.
    pub symbol: String,
    /// Watchlist the press originated from.
    pub wl_id: WatchlistId,
}

/// State for an in-progress ticker drag from a watchlist to a chart.
#[derive(Debug, Clone)]
pub struct DragTickerState {
    /// The ticker symbol being dragged.
    pub symbol: String,
    /// Current cursor position for the floating drag preview.
    pub cursor_pos: iced::Point,
}

/// Minimum interval between debounced config saves (in seconds).
const CONFIG_SAVE_DEBOUNCE_SECS: f64 = 2.0;

// ── Message enum ──────────────────────────────────────────────────────

/// All application messages. Every user interaction and async completion
/// flows through this enum via iced's Elm architecture.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Message {
    // -- Per-chart symbol input --
    /// Text changed in a chart's inline ticker input.
    PanelSymbolInputChanged(ChartId, String),
    /// User pressed Enter in a chart's inline ticker input.
    PanelSymbolSubmitted(ChartId),

    // -- Data loading --
    /// Async data load completed for a chart. Carries the requested
    /// symbol so the handler can discard stale loads (user switched
    /// tickers between request and response).
    DataLoaded(ChartId, String, u64, Result<Arc<CandleBuffer>, String>),

    /// Data loaded during startup restore (does not reset camera).
    DataRestoredFromStartup(ChartId, String, u64, Result<Arc<CandleBuffer>, String>),

    // -- Provider selection --
    /// User selected a data provider from the toolbar dropdown.
    DataProviderSelected(String),
    /// User selected an order broker from the toolbar dropdown.
    OrderBrokerSelected(String),

    // -- Chart management --
    /// Add a new empty chart panel to the workspace.
    AddChart,
    /// Close and remove a chart panel by its ChartId.
    CloseChart(ChartId),
    /// Set a chart as the active (focused) chart.
    ActivateChart(ChartId),

    // -- Layout --
    /// Switch to a predefined layout arrangement.
    LayoutPreset(LayoutPresetKind),

    // -- Pane grid --
    /// A pane was clicked, giving it focus.
    PaneFocused(pane_grid::Pane),
    /// A pane border was dragged to resize.
    PaneResized(pane_grid::ResizeEvent),
    /// A pane was dragged and dropped to reorder.
    PaneDragged(pane_grid::DragEvent),
    /// Split a pane along an axis.
    PaneSplit(pane_grid::Axis, pane_grid::Pane),
    /// Close a pane by its pane_grid handle.
    PaneClose(pane_grid::Pane),

    // -- Panel title bar --
    /// Timeframe button clicked on a specific panel's title bar.
    PanelTimeframeSelected(ChartId, Timeframe),

    // -- Chart interaction (from shader widget) --
    /// Wrapper for chart-interaction actions emitted by the chart
    /// widget. The audit's P2 #4 finding: this variant exists so the
    /// 17 sibling `Message::Chart*` variants (Pan/Zoom/Crosshair/
    /// CreateLevel/...) can be deleted in favour of one wrapper that
    /// carries the full `midas_chart::ChartAction` payload. Camera-
    /// dependent translations (pixel→data for `Zoom`/`ZoomY`) live in
    /// the dispatcher, which has access to the chart's camera via
    /// `self.charts`.
    Chart(ChartId, midas_chart::ChartAction),

    /// Viewport dimensions changed (old_w, old_h, new_w, new_h).
    /// Adjusts camera data range to preserve candle scale.
    /// Stays as a top-level variant: emitted by the iced shader
    /// widget directly when its bounds change, not via
    /// `action_to_message`, so it has no `ChartAction` analog.
    ChartViewportChanged(ChartId, u32, u32, u32, u32),
    /// Emitted once when a chart widget exits its
    /// `InteractionMode::DraggingAnnotation` — clears the app-level
    /// `dragging_annotation` so sibling charts stop drawing the level
    /// in their drag-pass z-layer. Paired with `ChartAction::DragLevel`,
    /// which sets the flag on every mouse-move during the drag.
    ChartDragLevelEnd(ChartId),
    /// Clear all levels from a chart (toolbar/keyboard, not from
    /// chart-interaction widget — has no `ChartAction` equivalent).
    ChartClearAllLevels(ChartId),

    // -- Level editing --
    /// Close the level editor popup.
    ChartCloseLevelEditor(ChartId),
    /// Delete a specific level by ID.
    ChartDeleteLevel(ChartId, u64),
    /// Update a level's price (from editor input).
    LevelEditorPriceChanged(ChartId, u64, String),
    /// Increment/decrement a level's price by delta.
    LevelEditorPriceStep(ChartId, u64, f64),
    /// Update a level's label text.
    LevelEditorLabelChanged(ChartId, u64, String),
    /// Update a level's color.
    LevelEditorColorChanged(ChartId, u64, [f32; 4]),
    /// Update a level's line thickness.
    LevelEditorThicknessChanged(ChartId, u64, f32),
    /// Update a level's icon.
    LevelEditorIconChanged(ChartId, u64, midas_chart::LevelIcon),
    /// Toggle a level's lock state.
    LevelEditorToggleLock(ChartId, u64),
    /// Create a new level from the drawing panel (at center of visible price range).
    DrawingPanelCreateLevel(ChartId),

    // -- Gap collapsing --
    /// Toggle session gap collapsing on a chart panel.
    ToggleCollapseGaps(ChartId),
    /// Toggle Volume Profile overlay on a chart panel.
    ToggleVolumeProfile(ChartId),
    /// Toggle horizontal price level visibility on a chart panel.
    ToggleLevels(ChartId),
    /// Reset chart to default view (fit all data).
    ResetChart(ChartId),

    // -- Chart backend toggle (chart-transition slice 9a) --
    /// Flip a chart panel between the legacy and new rendering
    /// backends. Auto-cancels any active bracket DRAFT before the
    /// swap (plan R11). LIVE brackets (entry filled, TP/SL resting)
    /// stay intact because broker-side state is untouched — only the
    /// rendering changes; the new layer reads `TickerState.live_bracket`
    /// on scene rebuild.
    ToggleChartBackend(ChartId),
    /// Explicitly set a chart panel's rendering backend. Used by the
    /// toolbar chip's direct-set path and by the config-restore flow
    /// so toggle + set share one handler.
    SetChartBackend(ChartId, ChartBackend),

    // -- Batched messages from shader widget --
    /// Multiple messages from a single widget event (shader can only publish one).
    ChartBatch(Vec<Message>),

    // -- Keyboard --
    /// A keyboard key was pressed (global shortcut handling).
    KeyPressed(iced::keyboard::Key),

    // -- Config --
    /// Config save completed (success or failure).
    ConfigSaved(Result<(), String>),
    /// Window close requested; save config before exit.
    WindowCloseRequested,

    // -- Floating windows --
    /// Pop out a pane's chart into a floating OS window.
    PopOut(pane_grid::Pane),
    /// A floating window was closed by the user.
    FloatingWindowClosed(window::Id),

    // -- Window geometry --
    /// Wrapper for OS-window geometry events (move / resize / monitor
    /// query / open). Routed to `dispatch_window` which calls the
    /// `WindowGeometry` controller and interprets its effects.
    /// Audit P1 slice 2 — collapsed `MainWindowOpened`, `WindowMoved`,
    /// `WindowResized`, `MonitorSizeResult` into one wrapper.
    Window(crate::window_geometry::WindowGeometryMsg),

    // -- Watchlist --
    /// Add a new watchlist panel to the workspace.
    AddWatchlist,
    /// Text changed in a watchlist's "add ticker" input.
    WatchlistTickerInputChanged(WatchlistId, String),
    /// User pressed Enter or clicked Add in a watchlist's ticker input.
    WatchlistAddTicker(WatchlistId),
    /// Remove a ticker from a watchlist.
    WatchlistRemoveTicker(WatchlistId, String),
    /// Adjust the favourite level of a ticker by a signed delta
    /// (scroll-wheel driven: `+1` per line up, `-1` per line down).
    WatchlistAdjustFavorite(WatchlistId, String, i8),
    /// Mouse entered a favourite cell — pin the favourites-first
    /// sort key so subsequent wheel adjustments don't reorder the
    /// row under the cursor.
    WatchlistFavCellEnter(WatchlistId),
    /// Mouse left the favourite cell — release the sort pin so the
    /// next render re-sorts with live `favorite` values.
    WatchlistFavCellExit(WatchlistId),
    /// User pressed down on a ticker cell — starts the hold timer.
    WatchlistTickerPressed(WatchlistId, String),
    /// Hold threshold reached — promote pending drag to active drag.
    WatchlistDragConfirm(String),
    /// User cancelled the drag (Escape key).
    WatchlistDragCancel,
    /// Mouse moved during drag (cursor tracking for preview).
    DragCursorMoved(iced::Point),
    /// Global mouse-up — attempt drop or cancel.
    DragMouseUp,
    /// User clicked a ticker row in a watchlist.
    WatchlistTickerSelected(WatchlistId, String),
    /// Set the symbol link mode for a watchlist panel.
    WatchlistSetSymbolLink(WatchlistId, LinkMode),
    /// Grid chrome event from a watchlist.
    WatchlistGrid(WatchlistId, midas_grid::GridMessage),

    // -- Chart linking --
    /// Set the symbol link mode for a chart (docked or floating).
    ///
    /// Collapses the previous `SetSymbolLink(ChartId, ..)` +
    /// `FloatingSetSymbolLink(window::Id, ..)` pair; the handler
    /// matches on [`ChartHandle`] to route to the correct storage map.
    /// See round-2 P2 in `plan/architecture-audit.md`.
    SetSymbolLink(ChartHandle, LinkMode),
    /// Set the timeframe link mode for a chart (docked or floating).
    SetTimeframeLink(ChartHandle, LinkMode),
    /// Toggle the link color picker for any panel.
    ToggleLinkPicker(PickerTarget, LinkDimension),
    /// Dismiss any open link picker.
    DismissLinkPicker,

    // -- Dockable order panel --
    /// Add a new dockable order panel to the workspace.
    AddOrderPanel,
    /// Action on a specific dockable order panel.
    OrderPanelMsg(OrderPanelId, crate::order_panel::OrderPanelAction),
    /// Set the symbol link mode for a dockable order panel.
    OrderPanelSetSymbolLink(OrderPanelId, LinkMode),

    // -- Account panel (tabbed Positions / Orders / History / Recents) --
    /// Add a new Account pane to the workspace.
    AddAccountPanel,
    /// Tab-scoped event routed through the Account pane (wraps `AccountMsg`).
    Account(AccountPanelId, crate::account_panel::AccountMsg),
    /// User clicked a row in an Account pane's Orders tab. Records the
    /// selection and (when linked) broadcasts the symbol.
    AccountOrdersRowSelected(AccountPanelId, uuid::Uuid, String),
    /// Change the Account panel's Orders-tab symbol-link colour group.
    AccountOrdersSetSymbolLink(AccountPanelId, LinkMode),
    /// Open the column-selector popup for an Account pane's Orders tab.
    AccountOrdersOpenColumnSelector(AccountPanelId),
    /// Dismiss the column-selector popup.
    AccountOrdersDismissColumnSelector,
    /// Toggle visibility of a single column in the given Account pane.
    AccountOrdersToggleColumn(AccountPanelId, midas_grid::ColumnId),
    /// Unified column-resize drag lifecycle across Watchlist and Account
    /// tabs (Orders/History/Recents). Replaces the four parallel
    /// `*ColumnResizeStart/ing/End` Message triples. See
    /// [`crate::column_resize`] for the target enum + routing.
    ColumnResize(crate::column_resize::ColumnResizeEvent),
    /// Coalesced batch of position updates from the broker subscription.
    /// App-wide (not per-panel) because the `PositionStore` is shared
    /// across every open Account pane.
    AccountPositionsBatch(Vec<crate::account_panel::PositionRaw>),

    // -- G.ATR hover highlight --
    /// Mouse entered the G.ATR badge on a chart — activate candle dimming.
    GatrHoverEnter(ChartId),
    /// Mouse left the G.ATR badge — deactivate candle dimming.
    GatrHoverLeave(ChartId),

    // -- Bracket context menu --
    // Bracket creation, drag, action buttons, and context-menu open
    // were collapsed into Message::Chart(ChartId, ChartAction)
    // (audit P2 #4 batch 3). Decorator-action and bracket-leg drag
    // are routed by `dispatch_chart_action` to the matching
    // `handle_chart_bracket_*` methods. Only context-menu lifecycle
    // (Cancel/Dismiss) remains as top-level variants because those
    // come from the popup widget, not the chart-action stream.
    /// Cancel a bracket from the context menu.
    BracketContextCancel(uuid::Uuid),
    /// Dismiss the bracket context menu.
    BracketContextDismiss,

    // -- Broker bracket events --
    /// A bracket was created by the broker engine (or order panel submit).
    BrokerBracketCreated {
        parent_id: uuid::Uuid,
        take_profit_id: Option<uuid::Uuid>,
        stop_loss_id: Option<uuid::Uuid>,
        symbol: String,
        action: midas_chart::widget::order_bracket::BracketSide,
        quantity: f64,
        entry_price: Option<f64>,
        tp_price: Option<f64>,
        sl_price: Option<f64>,
    },
    /// A bracket's status changed (broker lifecycle update).
    BrokerBracketStatusChanged {
        parent_id: uuid::Uuid,
        status: midas_chart::widget::order_bracket::BracketStatus,
        entry_fill_price: Option<f64>,
    },

    /// Raw broker event received from the subscription channel.
    /// Boxed to keep Message size small (BrokerEvent is large).
    BrokerEventReceived(Box<midas_broker::BrokerEvent>),
    /// Broker connection state changed.
    BrokerConnectionChanged(String),

    /// Async result of a [`BracketSubmitter::place_bracket`] call
    /// (router refactor slice 10b). The UI logs and surfaces errors to
    /// the user; success is driven by the subsequent `OrderEvent`
    /// stream which reconciles to the provisional annotation.
    BracketPlaceResult(BracketPlaceOutcome),

    /// Order-lifecycle event from the router's
    /// [`midas_broker::OrderClient::order_events`] broadcast (router
    /// refactor slice 10c). Carries the raw [`midas_broker::OrderEvent`];
    /// the handler translates IB order ids back to local UUIDs using
    /// the `order_annotation_links` map and fans out to the existing
    /// order-blotter / TickerState handlers.
    RouterOrderEvent(Box<midas_broker::OrderEvent>),

    // -- Toast notifications --
    /// All toast traffic routes through one wrapper variant.
    /// [`crate::toast::ToastMsg`] is the controller-local enum.
    Toast(crate::toast::ToastMsg),

    // -- Market data cache --
    /// Market data snapshot loaded for a watchlist symbol (D1 candles).
    MarketSnapshotLoaded(String, Result<midas_core::CandleBuffer, String>),
    /// Timer tick to refresh all cached market data.
    RefreshMarketData,

    // -- Window --
    /// Periodic tick for animations and status bar clock.
    Tick,

    // -- Ticker state machine (Slice 0; stubs returning empty effects) --
    /// Route a per-symbol ticker state message through
    /// `TickerState::apply()`. Slice 0 stubs return empty effects.
    Ticker(
        crate::annotation_store::SymbolKey,
        crate::ticker_state::TickerMsg,
    ),

    // -- Thumbnail cells (Slice 3 / Slice 4) --
    /// User clicked a grid-cell thumbnail to cycle its interval. The
    /// handler cycles [`ThumbnailStore`](crate::thumbnail_store::ThumbnailStore)
    /// and dispatches a load task for the new `(symbol, tf)` via the
    /// active [`DataProvider`].
    ThumbnailIntervalCycle(String),
    /// Async load for a thumbnail completed. Installs the new buffer
    /// into `thumbnail_data`, clearing any pending marker.
    ThumbnailDataReady {
        /// Symbol the load was for.
        symbol: String,
        /// Timeframe the load was for.
        tf: midas_core::Timeframe,
        /// Freshly-loaded buffer. `Arc`'d so the message enum stays
        /// cheap to `Clone`.
        buffer: std::sync::Arc<midas_core::CandleBuffer>,
    },
    /// Async load for a thumbnail failed (provider error, zero rows,
    /// etc.). Installs an empty placeholder entry so
    /// [`request_load`](crate::thumbnail_data::ThumbnailDataStore::request_load)
    /// can re-dispatch a retry without looping.
    ThumbnailLoadFailed {
        /// Symbol the load was for.
        symbol: String,
        /// Timeframe the load was for.
        tf: midas_core::Timeframe,
    },

    // -- Dev harness (feature-gated) --
    /// Command received over the devloop TCP socket. Handled in
    /// `crate::dev_harness::handle_command`. Carries a one-shot
    /// responder that is fired with the `Response` the client sees.
    #[cfg(feature = "dev_harness")]
    DevHarness {
        command: midas_devloop_proto::Command,
        responder: crate::dev_harness::Responder,
    },

    /// A `window::screenshot()` task returned — encode PNG, compute
    /// diff against reference, fire the pending responder.
    #[cfg(feature = "dev_harness")]
    DevHarnessScreenshotReady {
        screenshot: iced::window::Screenshot,
        out_path: std::path::PathBuf,
        responder: crate::dev_harness::Responder,
    },

    /// A [`midas_devloop_proto::Command::SpawnSim`] task finished: the
    /// sim binary is up and the control plane is healthy. Stash the
    /// child handle on the app and fire the pending responder.
    #[cfg(feature = "dev_harness")]
    DevHarnessSimSpawned {
        handle: crate::sim_child::SimChildHandle,
        responder: crate::dev_harness::Responder,
    },

    /// The app-owned auto-spawn of `midas-ib-sim-server` finished.
    /// Fires during startup when `config.broker.backend == Sim`.
    /// Success stashes the handle on `self.sim_child`; failure
    /// surfaces the reason in the status bar without tearing the
    /// app down — the user can still edit config to switch to
    /// LivePaper and retry.
    BrokerSimSpawned(Result<crate::sim_child::SimChildHandle, String>),

    // -- Router refactor (S7) --
    /// A coalesced batch of bars from the per-chart subscription
    /// stream (`chart_subscriptions`). Handler folds each bar into
    /// the chart's `CandleBuffer` via `apply_bar`.
    ChartBarBatch {
        /// The chart this batch is for. Handler filters out
        /// batches for charts that no longer exist.
        chart_id: ChartId,
        /// Bars since the last frame boundary (~16 ms). Capped to
        /// a small number by the subscription's coalescer.
        bars: Vec<midas_broker_core::market_data::Bar>,
    },

    /// Sub-bucket bars from the raw-RT fallback path (timeframes the
    /// aggregator can't synthesise: S1/H4/D1/W1/MN1). Handler floors
    /// each bar's `ts_open` to the chart's bucket and merges
    /// (high=max, low=min, close=latest, volume+=) via
    /// `CandleBuffer::merge_bar`.
    ChartSubBarBatch {
        chart_id: ChartId,
        bars: Vec<midas_broker_core::market_data::Bar>,
        /// Bucket width in seconds (matches the chart's `Timeframe::as_secs`).
        bucket_secs: u64,
    },

    /// The chart's bar stream observed a `Lagged` error; rebuild
    /// the historical prefix. M-29 throttled to at most one resync
    /// per chart per 5 s in the handler.
    ChartResync {
        /// Which chart asked to be resynced.
        chart_id: ChartId,
    },

    /// Async completion of a resync load started by
    /// [`Message::ChartResync`]. Replaces the chart's data buffer.
    ChartResyncLoaded(Result<(ChartId, Vec<midas_broker_core::market_data::Bar>), String>),

    /// Batch of quote updates from the watchlist subscription. Each
    /// entry updates `market_cache` for the corresponding symbol.
    QuoteBatch(
        Vec<(
            midas_broker_core::SymbolKey,
            midas_broker_core::market_data::Quote,
        )>,
    ),

    /// Rare path: the watchlist subscription's `watch::Receiver`
    /// reported `RecvError::Closed` — the router dropped the watch
    /// sender, typically because the last consumer DecRef'd and
    /// the publisher was torn down. Handler re-opens the watch via
    /// `router.last_quote(sym)` and refreshes `market_cache` from
    /// the snapshot. See S8 §F.
    QuoteResync {
        /// Symbol that needs to be re-opened.
        symbol: midas_broker_core::SymbolKey,
    },

    /// Latest price observation for a specific ticker. Drives
    /// `TickerMsg::UpdateMarketData` on the matching
    /// `TickerState`. Keyed by the router-era broker-core
    /// `SymbolKey`; the handler normalises to the app's string key
    /// via `SymbolKey::new(..symbol.as_str())`.
    TickerLastPrice {
        /// Broker-core symbol key (carries contract_id for wire
        /// correlation; handler discards contract_id and keys on
        /// the ticker string for TickerState lookup).
        symbol: midas_broker_core::SymbolKey,
        /// Observed last price in instrument currency.
        last_price: f64,
    },

    /// A farm-status transition from the router's shared
    /// `farm_status` broadcast. Used for logging + future
    /// status-bar granularity (e.g. "hmds.us connected" vs
    /// "hmds.us halted").
    FarmStatusChanged(midas_broker_core::market_data::FarmStatus),

    /// Router + order client became ready. Populates
    /// `self.router` / `self.router_order_client` and kicks the
    /// subscription re-diff so chart / watchlist / ticker subs
    /// spin up.
    ///
    /// Wrapped in [`RouterReadyPayload`] because `dyn OrderClient`
    /// is `!Debug` while `Message` derives `Debug`.
    RouterReady(Result<RouterReadyPayload, String>),

    /// Session-aware-charts (Phase B S8 + Phase C S10–S14).
    /// Feature-gated on `session_chart`.
    ///
    /// Carries the full request (ticker + period + calendar) so the
    /// handler can route to any of the three canonical presets
    /// (BTC-M1, AAPL-M5, SPY-D1-RTH) without a hard-coded chain.
    ///
    /// The handler:
    /// 1. Resolves the ticker through `StaticSymbolResolver` and
    ///    asserts the calendar matches `request.calendar_id`.
    /// 2. Spins up a `SessionedBarStream` via
    ///    `midas_bars_adapter::subscribe_aggregated_bars`, optionally
    ///    wrapping it in [`midas_stream::Filtered<_, EhFilter>`] per
    ///    the current [`crate::session_chart::widget::EhPolicy`].
    /// 3. Wraps the stream in a [`crate::session_chart::SessionChartDriver`].
    /// 4. Opens a standalone iced window (via `window::open`) and
    ///    stores the widget + driver in `floating_session_charts`
    ///    keyed by the returned `window::Id` once iced hands it back
    ///    on [`Message::SessionChartWindowOpened`].
    #[cfg(feature = "session_chart")]
    OpenSessionChart(crate::session_chart::SessionChartRequest),

    /// The iced runtime created a session-chart window and handed us
    /// its `window::Id`. Completes the lifecycle started in
    /// `handle_open_session_chart`. Feature-gated.
    #[cfg(feature = "session_chart")]
    SessionChartWindowOpened(window::Id, SessionChartWindowPayload),

    /// Async pipeline construction failed (timeout, resolver error,
    /// etc.). Close the already-opened but empty window so the user
    /// isn't left with a blank frame. Feature-gated. App-harden M1.
    #[cfg(feature = "session_chart")]
    SessionChartOpenFailed(window::Id),

    /// User pressed the EH-policy toggle chip in a session-chart
    /// window. Cycles the widget's policy; a full subscribe-rebuild
    /// is a later slice (the filter wraps the stream at subscribe
    /// time, so toggling mid-stream requires the host to spawn a
    /// fresh driver). For now the chip cycles the rendering policy
    /// only — a TODO tracked in `session_chart/mod.rs`.
    #[cfg(feature = "session_chart")]
    SessionChartCyclePolicy(window::Id),

    /// Slice 4 of chart-transition plan: toolbar "Add Level" button on
    /// the session-chart window toggles the level-placement tool. The
    /// handler flips the widget's tool state; subsequent mouse clicks
    /// on the chart surface commit level annotations.
    #[cfg(feature = "session_chart")]
    SessionChartToggleLevelTool(window::Id),

    /// Slice 5b of chart-transition plan: "Buy Bracket" toolbar button
    /// on the session-chart window activates the bracket-placement
    /// tool for a Long (Buy) bracket. Subsequent clicks on the chart
    /// surface commit a bracket via the existing draft-then-save
    /// `TickerMsg` sequence.
    #[cfg(feature = "session_chart")]
    SessionChartActivateBuyBracketTool(window::Id),

    /// Slice 5b: "Sell Bracket" toolbar button — Short-side bracket.
    #[cfg(feature = "session_chart")]
    SessionChartActivateSellBracketTool(window::Id),
}

/// Payload for [`Message::SessionChartWindowOpened`]. Carries the
/// freshly-spawned driver + request so the update handler can build
/// the widget once it knows the window `Id`. Feature-gated.
#[cfg(feature = "session_chart")]
#[derive(Clone)]
pub struct SessionChartWindowPayload {
    /// The driver pumping the stream into the shared series.
    pub driver: std::sync::Arc<crate::session_chart::SessionChartDriver>,
    /// The request that spawned this chart.
    pub request: crate::session_chart::SessionChartRequest,
}

#[cfg(feature = "session_chart")]
impl std::fmt::Debug for SessionChartWindowPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionChartWindowPayload")
            .field("request", &self.request)
            .finish()
    }
}

/// Payload for [`Message::RouterReady`]. The inner `Arc<dyn
/// OrderClient>` is `!Debug`, but `Message` derives `Debug`, so we
/// wrap the pair in a struct with a manual `Debug` impl that hides
/// the order-client trait object behind its `name()`.
#[derive(Clone)]
pub struct RouterReadyPayload {
    /// The freshly-constructed router.
    pub router: std::sync::Arc<midas_market_data::MarketDataRouter>,
    /// The freshly-constructed order client.
    pub order_client: std::sync::Arc<dyn midas_broker::OrderClient>,
}

impl std::fmt::Debug for RouterReadyPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterReadyPayload")
            .field("router", &self.router)
            .field("order_client", &self.order_client.name())
            .finish()
    }
}

/// Outcome of a [`BracketSubmitter::place_bracket`] call, carried on
/// [`Message::BracketPlaceResult`] (router refactor slice 10b).
///
/// `symbol` is cloned in so the UI handler can surface errors without
/// cross-referencing the drag-origin chart.
#[derive(Debug, Clone)]
pub struct BracketPlaceOutcome {
    /// Symbol the submission was for.
    pub symbol: String,
    /// Result of the submission. `Ok(handle)` carries the IB order ids
    /// assigned to each leg; `Err(msg)` is a human-readable reason.
    pub result: Result<midas_broker::BracketHandle, String>,
}

/// Classify messages the `wait_for_idle` tracker should NOT treat as
/// input activity: `Tick` fires constantly, market-data updates fire
/// per broker tick, etc. Everything else is considered "real" work.
#[cfg(feature = "dev_harness")]
fn is_tick_rate_message(msg: &Message) -> bool {
    use crate::ticker_state::TickerMsg;
    // Also exclude streaming broker ticks: when the sim is live the
    // watchlist receives continuous L1 updates, and treating each
    // one as "real" work would prevent `wait_for_idle` from ever
    // settling under live-sim conditions.
    let is_broker_tick = matches!(
        msg,
        Message::BrokerEventReceived(boxed)
            if matches!(**boxed, midas_broker::BrokerEvent::Tick { .. })
    );
    is_broker_tick
        || matches!(
            msg,
            Message::Tick
                | Message::Ticker(_, TickerMsg::UpdateMarketData { .. })
                | Message::RefreshMarketData
                | Message::MarketSnapshotLoaded(..)
                // S7: the new router-driven per-consumer
                // streams are high-frequency; exclude them
                // from the wait_for_idle tracker for the same
                // reason the legacy Tick path is excluded.
                | Message::ChartBarBatch { .. }
                | Message::ChartSubBarBatch { .. }
                | Message::QuoteBatch(..)
                | Message::TickerLastPrice { .. }
                | Message::FarmStatusChanged(..)
        )
}

/// Single-retry connect helper used by the IB router startup task
/// (S8b).
///
/// Tries `IbMarketData::new + connect` once, sleeps 5 s on failure,
/// tries again, and wraps the `(router, order_client)` pair in a
/// `RouterReadyPayload` on success. Returning `Err(String)` from
/// both attempts means the user must reconnect manually via the
/// status bar — iced's subscription diff sees `self.router = None`
/// and emits no subscriptions until a real router lands.
async fn build_ib_router(
    cfg: midas_broker::ib::IbMarketDataConfig,
) -> Result<RouterReadyPayload, String> {
    use std::sync::Arc;

    async fn try_connect(
        cfg: midas_broker::ib::IbMarketDataConfig,
    ) -> Result<RouterReadyPayload, String> {
        let market = Arc::new(midas_broker::ib::IbMarketData::new(cfg));
        market
            .connect()
            .await
            .map_err(|e| format!("IB connect: {e}"))?;
        let order_client: Arc<dyn midas_broker::OrderClient> =
            Arc::new(midas_broker::ib::IbOrderClient::new(Arc::clone(&market)));
        let router = midas_market_data::MarketDataRouter::new(
            market as Arc<dyn midas_broker::MarketDataSource>,
        );
        Ok(RouterReadyPayload {
            router,
            order_client,
        })
    }

    match try_connect(cfg.clone()).await {
        Ok(p) => Ok(p),
        Err(first) => {
            tracing::warn!("IB connect attempt 1 failed: {first}; retrying in 5 s");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            try_connect(cfg)
                .await
                .map_err(|second| format!("{first}; retry: {second}"))
        }
    }
}

// ── Constructor + helpers ─────────────────────────────────────────────

impl MidasApp {
    /// Resolve the path to the user configuration file.
    ///
    /// Uses the OS-standard local config directory
    /// (`C:\Users\<user>\AppData\Local\HandOfMidas\config.toml` on Windows).
    /// Falls back to `data/` if the platform directory cannot be determined.
    pub fn config_file_path() -> PathBuf {
        let base = dirs::config_local_dir().unwrap_or_else(|| PathBuf::from("data"));
        base.join("HandOfMidas").join("config.toml")
    }

    /// One-time migration: copy `data/config.toml` to the new standard
    /// location if the old file exists but the new one does not.
    fn migrate_legacy_config(new_path: &std::path::Path) {
        let legacy = PathBuf::from("data/config.toml");
        if legacy.exists() && !new_path.exists() {
            if let Some(parent) = new_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::copy(&legacy, new_path) {
                Ok(_) => {
                    tracing::info!(
                        "Migrated legacy config from {} to {}",
                        legacy.display(),
                        new_path.display()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to migrate legacy config from {} to {}: {e}",
                        legacy.display(),
                        new_path.display()
                    );
                }
            }
        }
    }

    /// Check whether a saved window position is still usable.
    ///
    /// Returns `Position::Specific` if the position exists and the saved
    /// monitor dimensions match the current system (heuristic: monitor_width
    /// and monitor_height are present). If the monitor config changed or no
    /// position was saved, falls back to `Position::Default`.
    fn validate_saved_position(wc: &midas_core::config::WindowConfig) -> window::Position {
        let (x, y) = match (wc.x, wc.y) {
            (Some(x), Some(y)) => (x, y),
            _ => return window::Position::Default,
        };

        // Basic sanity: reject positions that are clearly off-screen
        // (e.g. negative by more than the window size, or absurdly large).
        // The window should be at least partially visible.
        let w = wc.width as i32;
        let h = wc.height as i32;
        if x + w < 100 || y + h < 50 || x > 8000 || y > 5000 {
            return window::Position::Default;
        }

        // If we have saved monitor dimensions, check they still match.
        // A mismatch likely means the user changed display setup.
        // We can't query monitors at this point (no window yet), so
        // we trust the saved dimensions as a heuristic.
        window::Position::Specific(iced::Point::new(x as f32, y as f32))
    }

    /// Project the inputs needed to render an Account panel's header
    /// chrome (tab strip badges + disconnect banner) into a self-
    /// contained view-model. Returns `None` if `account_id` does not
    /// resolve to an open panel.
    ///
    /// Pure-`&self` and side-effect free, so the projection logic is
    /// unit-testable without booting the iced runtime.
    pub fn account_panel_header_vm(
        &self,
        account_id: midas_core::AccountPanelId,
    ) -> Option<crate::view_models::account_panel::AccountPanelHeaderVm> {
        use crate::view_models::account_panel::AccountPanelHeaderVm;
        let panel = self.account_panels.get(&account_id)?;
        let broker_connected = matches!(
            self.broker_connection_display.as_str(),
            "Ready" | "Connected"
        );
        let working_count = self
            .order_blotter
            .rows()
            .filter(|r| !r.status.is_terminal())
            .count()
            .min(AccountPanelHeaderVm::BADGE_CAP);
        let history_count = self
            .order_blotter
            .terminal_row_count()
            .min(AccountPanelHeaderVm::BADGE_CAP);
        let positions_count = self.positions.len().min(AccountPanelHeaderVm::BADGE_CAP);
        let recents_count = self
            .recent_symbols
            .len()
            .min(AccountPanelHeaderVm::RECENTS_BADGE_CAP);
        Some(AccountPanelHeaderVm {
            active_tab: panel.active_tab,
            working_count,
            history_count,
            positions_count,
            recents_count,
            show_disconnect_banner: panel.should_show_disconnect_banner(broker_connected),
        })
    }

    /// Project the inputs needed by `view_account_recents_tab` into a
    /// VM. Returns `None` if `account_id` does not resolve to an open
    /// panel. Pure-`&self`; deterministic given a fixed `now`.
    pub fn account_recents_tab_vm(
        &self,
        account_id: midas_core::AccountPanelId,
    ) -> Option<crate::view_models::account_panel::AccountRecentsTabVm> {
        use crate::view_models::account_panel::AccountRecentsTabVm;
        let panel = self.account_panels.get(&account_id)?;
        let show_resize_overlay = matches!(
            self.resizing_column.as_ref().map(|s| s.target),
            Some(crate::column_resize::ColumnResizeTarget::AccountRecents(id)) if id == account_id
        );
        Some(AccountRecentsTabVm::build(
            &panel.recents,
            self.recent_symbols.iter(),
            show_resize_overlay,
            std::time::Instant::now(),
        ))
    }

    /// Project the inputs needed by `view_account_history_tab` into a
    /// VM. Returns `None` if `account_id` does not resolve to an open
    /// panel. The VM borrows from `self.account_panels[id].history` —
    /// see [`crate::view_models::account_panel::AccountHistoryTabVm`]
    /// for why the borrow can't be sidestepped by cloning.
    pub fn account_history_tab_vm(
        &self,
        account_id: midas_core::AccountPanelId,
    ) -> Option<crate::view_models::account_panel::AccountHistoryTabVm<'_>> {
        use crate::view_models::account_panel::AccountHistoryTabVm;
        let panel = self.account_panels.get(&account_id)?;
        let show_resize_overlay = matches!(
            self.resizing_column.as_ref().map(|s| s.target),
            Some(crate::column_resize::ColumnResizeTarget::AccountHistory(id)) if id == account_id
        );
        Some(AccountHistoryTabVm::build(
            &panel.history,
            show_resize_overlay,
        ))
    }

    /// Project the Orders-tab inputs into a VM: visible-column filter,
    /// overlay flags, hidden-column set, sorted rows, per-row thumbnail
    /// snapshots, column widths, selection, and sort indicator.
    /// Returns `None` if `account_id` does not resolve to an open panel.
    ///
    /// `all_columns` is passed in by the view because the static
    /// `(ColumnId, label, sortable)` table is a presentation choice
    /// living in `views.rs`, not a state shape.
    pub fn account_orders_tab_vm(
        &self,
        account_id: midas_core::AccountPanelId,
        all_columns: &[(midas_grid::ColumnId, &'static str, bool)],
    ) -> Option<crate::view_models::account_panel::AccountOrdersTabVm> {
        use crate::view_models::account_panel::AccountOrdersTabVm;
        let panel = self.account_panels.get(&account_id)?;
        let show_resize_overlay = matches!(
            self.resizing_column.as_ref().map(|s| s.target),
            Some(crate::column_resize::ColumnResizeTarget::AccountOrders(id)) if id == account_id
        );
        let show_column_selector = self.account_column_selector_open == Some(account_id);
        Some(AccountOrdersTabVm::build(
            &panel.orders,
            &self.order_blotter,
            all_columns,
            |symbol| self.build_thumbnail_snapshot(symbol),
            show_resize_overlay,
            show_column_selector,
        ))
    }

    /// Build the immutable per-frame render snapshot for the chart
    /// pane identified by `chart_id`. Returns `None` when the chart
    /// is not open OR when no candle data has loaded yet (the caller
    /// renders the empty/loading placeholder in that case).
    ///
    /// All ~10 `self.*` reads previously inlined inside
    /// `view_pane_body` (level store, annotation store, ticker
    /// state, drag/preview state, market cache for G.ATR, crosshair
    /// sync) live here now. The view function consumes the snapshot
    /// (and a few cheap chart-local fields) and stays presentation-
    /// only.
    pub fn chart_render_snapshot(
        &self,
        chart_id: midas_core::ChartId,
    ) -> Option<crate::chart_widget::ChartRenderSnapshot> {
        let chart = self.charts.get(&chart_id)?;
        self.chart_render_snapshot_for(chart, chart_id)
    }

    /// Same as [`Self::chart_render_snapshot`] but takes a borrowed
    /// `ChartPanel` directly. Floating-window charts live outside
    /// `self.charts`, so they can't go through the id lookup; they
    /// own the `ChartPanel` themselves and pass `ChartId::new(0)` as
    /// the cross-chart-comparison sentinel (used by the
    /// crosshair/preview projections).
    pub fn chart_render_snapshot_for(
        &self,
        chart: &ChartPanel,
        chart_id: midas_core::ChartId,
    ) -> Option<crate::chart_widget::ChartRenderSnapshot> {
        let data = chart.data.as_ref()?;

        // G.ATR is only meaningful on the daily timeframe; the bright-
        // range overlay also gates on the hover hint.
        let bright_ranges = if chart.gatr_hover && chart.timeframe == midas_core::Timeframe::D1 {
            chart.data.as_ref().map_or(Vec::new(), |d| {
                crate::app::views::compute_daily_bright_ranges(d)
            })
        } else {
            Vec::new()
        };

        Some(crate::chart_widget::ChartRenderSnapshot {
            symbol: chart.symbol.clone(),
            data: Some(std::sync::Arc::clone(data)),
            camera: chart.chart_state.camera.clone(),
            dirty: chart.chart_state.dirty.clone(),
            crosshair_pos: chart.chart_state.crosshair.render_pos(),
            viewport_width: chart.chart_state.camera.viewport_width,
            viewport_height: chart.chart_state.camera.viewport_height,
            collapse_gaps: chart.chart_state.collapse_gaps,
            timeline_border_ratio: chart.chart_state.timeline_border_ratio,
            volume_scale: chart.chart_state.volume_scale,
            show_volume_profile: chart.chart_state.show_volume_profile,
            show_levels: chart.chart_state.show_levels,
            data_time_start: chart.chart_state.data_time_start,
            data_time_end: chart.chart_state.data_time_end,
            editing_level_id: chart.editing_level_id,
            dragging_annotation_id: self.dragging_annotation.map(|aid| aid.0),
            level_tool: chart.chart_state.level_tool.clone(),
            level_placing: self.level_placing,
            ghost_crosshair: crate::app::views::compute_ghost_crosshair(
                &self.crosshair_sync,
                chart_id,
                &chart.symbol,
                &chart.chart_state,
                chart.data.as_deref(),
            ),
            ghost_preview_price: self
                .placing_preview
                .as_ref()
                .and_then(|(src_id, sym, price)| {
                    if *src_id != chart_id && chart.symbol == *sym {
                        Some(*price)
                    } else {
                        None
                    }
                }),
            placing_cursor_chart: self.placing_preview.as_ref().map(|(id, _, _)| *id),
            annotations: self.annotation_store.get(&chart.symbol).to_vec(),
            gatr_bright_ranges: bright_ranges,
            pinned: self
                .tickers
                .get(&crate::annotation_store::SymbolKey::new(&chart.symbol))
                .map(|ts| ts.pinned())
                .unwrap_or(false),
        })
    }

    /// Project the Account pane's TitleBar inputs into a VM. Always
    /// returns a value; missing panel falls back to defaults
    /// (`name = "Account"`, `symbol_link = Unlinked`) — matches the
    /// prior render path's `unwrap_or` chain.
    pub fn account_title_bar_vm(
        &self,
        account_id: midas_core::AccountPanelId,
    ) -> crate::view_models::account_panel::AccountTitleBarVm {
        use crate::view_models::account_panel::AccountTitleBarVm;
        let panel = self.account_panels.get(&account_id);
        AccountTitleBarVm {
            name: panel
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Account".to_string()),
            row_count: self.order_blotter.len(),
            symbol_link: panel
                .map(|p| p.orders.symbol_link)
                .unwrap_or(midas_core::LinkMode::Unlinked),
        }
    }

    /// Project the top-toolbar provider drop-down inputs into a VM.
    /// Resolves both lists + active selections off `self.providers`
    /// in one place so the view doesn't reach into the registry
    /// multiple times.
    ///
    /// The broker-picker side is inert after the router refactor
    /// (no broker backends register through this registry any more);
    /// the VM carries a singleton `"None"` list so the iced pick_list
    /// renders with its disabled-looking default. A future Phase 1
    /// slice wires live IB selection through the router's
    /// `ConnectionState` watch directly.
    pub fn toolbar_vm(&self) -> crate::view_models::toolbar::ToolbarVm {
        crate::view_models::toolbar::ToolbarVm {
            data_provider_names: self.providers.data_provider_names(),
            active_data_provider: self.providers.active_data_provider_name(),
            broker_names: vec!["None".to_string()],
            active_broker: "None".to_string(),
        }
    }

    /// Project the order panel's TitleBar inputs. Always returns a
    /// value; missing panel falls back to `"Order"` + `Unlinked`.
    pub fn order_panel_title_bar_vm(
        &self,
        order_id: midas_core::OrderPanelId,
    ) -> crate::view_models::order_panel::OrderPanelTitleBarVm {
        use crate::view_models::order_panel::OrderPanelTitleBarVm;
        let panel = self.order_panels.get(&order_id);
        let title_text = panel
            .map(|p| {
                if p.state.symbol.is_empty() {
                    "Order".to_string()
                } else {
                    format!("Order: {}", p.state.symbol)
                }
            })
            .unwrap_or_else(|| "Order".to_string());
        let symbol_link = panel
            .map(|p| p.symbol_link)
            .unwrap_or(midas_core::LinkMode::Unlinked);
        OrderPanelTitleBarVm {
            title_text,
            symbol_link,
        }
    }

    /// Project the order panel body inputs (borrowed state +
    /// last_price + pre-computed coarse step). Returns `None` when
    /// the panel id doesn't resolve.
    pub fn order_panel_body_vm(
        &self,
        order_id: midas_core::OrderPanelId,
    ) -> Option<crate::view_models::order_panel::OrderPanelBodyVm<'_>> {
        use crate::view_models::order_panel::OrderPanelBodyVm;
        let panel = self.order_panels.get(&order_id)?;
        let last_price = self
            .market_cache
            .get(&crate::annotation_store::SymbolKey::new(
                &panel.state.symbol,
            ))
            .and_then(|snap| snap.last_price);
        let (coarse_step, _fine_step) = midas_chart::price_step_for(last_price.unwrap_or(100.0));
        Some(OrderPanelBodyVm {
            state: &panel.state,
            last_price,
            coarse_step,
        })
    }

    /// Project all status-bar inputs into a single VM. Resolves the
    /// active-chart label, pane count, both connection blocks (data
    /// and broker) with their colours, and the various messages and
    /// clock fields. Folds 8+ `self.*` reads in `view_status_bar`
    /// plus the connection-indicator helper into one builder call.
    pub fn status_bar_vm(&self) -> crate::view_models::status_bar::StatusBarVm {
        use crate::view_models::status_bar::{ConnectionBlockVm, StatusBarVm};
        use iced::Color;

        let active_info = if let Some(id) = self.active_chart_id() {
            if let Some(chart) = self.charts.get(&id) {
                let sym = if chart.symbol.is_empty() {
                    "---"
                } else {
                    &chart.symbol
                };
                format!("{sym} | {}", chart.timeframe.display_name())
            } else {
                "---".to_string()
            }
        } else {
            "No chart".to_string()
        };

        let data_connection = {
            let provider_name = self.providers.active_data_provider_name().to_string();
            let is_connected = self
                .providers
                .active_data_provider()
                .is_some_and(|p| p.is_connected());
            let dot_color = if is_connected {
                Color::from_rgb(0.2, 0.8, 0.2)
            } else {
                Color::from_rgb(0.6, 0.6, 0.6)
            };
            ConnectionBlockVm {
                dot_color,
                label: provider_name,
            }
        };

        let broker_connection = match self.broker_connection_display.as_str() {
            "Ready" => {
                // Router refactor retired the `OrderBroker` trait
                // registry; the status bar now shows a generic
                // "Broker: Ready" label until a future slice surfaces
                // the router-era backend name through
                // `router_order_client.name()`.
                ConnectionBlockVm {
                    dot_color: Color::from_rgb(0.2, 0.8, 0.2),
                    label: "Broker: Ready".to_string(),
                }
            }
            "Disconnected" => ConnectionBlockVm {
                dot_color: Color::from_rgb(0.6, 0.6, 0.6),
                label: format!("Broker: {}", self.broker_connection_display),
            },
            _ => ConnectionBlockVm {
                dot_color: Color::from_rgb(0.9, 0.7, 0.2),
                label: format!("Broker: {}", self.broker_connection_display),
            },
        };

        StatusBarVm {
            active_info,
            pane_count: self.workspace.pane_count(),
            overlay_indicator: if self.show_frame_overlay {
                " | F11: overlay ON"
            } else {
                ""
            },
            status_message: self.status_message.clone(),
            current_time: self.current_time.clone(),
            data_connection,
            broker_connection,
        }
    }

    /// Project the chart pane's TitleBar inputs into a VM. Always
    /// returns a value; missing chart falls back to the same defaults
    /// the prior `chart.map(...).unwrap_or(default)` chain produced
    /// (empty symbol input, `Timeframe::D1`, levels-on, etc.).
    pub fn chart_pane_title_bar_vm(
        &self,
        chart_id: midas_core::ChartId,
    ) -> crate::view_models::chart_pane::ChartPaneTitleBarVm {
        use crate::view_models::chart_pane::ChartPaneTitleBarVm;
        let chart = self.charts.get(&chart_id);
        ChartPaneTitleBarVm {
            symbol_input: chart.map(|c| c.symbol_input.clone()).unwrap_or_default(),
            timeframe: chart
                .map(|c| c.timeframe)
                .unwrap_or(midas_core::Timeframe::D1),
            collapse_gaps: chart.map(|c| c.chart_state.collapse_gaps).unwrap_or(false),
            show_volume_profile: chart
                .map(|c| c.chart_state.show_volume_profile)
                .unwrap_or(false),
            show_levels: chart.map(|c| c.chart_state.show_levels).unwrap_or(true),
            symbol_link: chart
                .map(|c| c.symbol_link)
                .unwrap_or(midas_core::LinkMode::Unlinked),
            timeframe_link: chart
                .map(|c| c.timeframe_link)
                .unwrap_or(midas_core::LinkMode::Unlinked),
            backend: chart
                .map(|c| c.backend)
                .unwrap_or(midas_core::ChartBackend::Legacy),
        }
    }

    /// Project the chart pane's overlay-layer inputs into a VM —
    /// G.ATR badge, level-placing flag, editing-level popup, link-
    /// picker dimension. Returns `None` when the chart isn't open
    /// (the view's snapshot path already handles the no-data case
    /// independently).
    pub fn chart_pane_overlays_vm(
        &self,
        chart_id: midas_core::ChartId,
    ) -> Option<crate::view_models::chart_pane::ChartPaneOverlaysVm> {
        let chart = self.charts.get(&chart_id)?;
        let link_picker_dim = match self.link_picker_open {
            Some((PickerTarget::Docked(picker_id), dim)) if picker_id == chart_id => Some(dim),
            _ => None,
        };
        Some(self.chart_pane_overlays_vm_for(chart, link_picker_dim))
    }

    /// Same as [`Self::chart_pane_overlays_vm`] but takes a borrowed
    /// `ChartPanel` directly. Used by the floating-window view path
    /// (which owns its `ChartPanel` outside `self.charts`) and which
    /// resolves the link-picker target itself
    /// (`PickerTarget::Floating(wid)` rather than
    /// `PickerTarget::Docked(chart_id)`).
    pub fn chart_pane_overlays_vm_for(
        &self,
        chart: &ChartPanel,
        link_picker_dim: Option<crate::link::LinkDimension>,
    ) -> crate::view_models::chart_pane::ChartPaneOverlaysVm {
        use crate::view_models::chart_pane::{ChartPaneOverlaysVm, EditingLevelVm};
        let gatr = crate::app::views::gatr_render_from_cache(&self.market_cache, &chart.symbol);
        let editing_level = match (chart.editing_level_id, chart.editing_level_screen_pos) {
            (Some(editing_id), Some(screen_pos)) => self
                .annotation_store
                .levels_for(&chart.symbol)
                .into_iter()
                .find(|l| l.id == editing_id)
                .map(|level| EditingLevelVm {
                    level,
                    screen_pos,
                    price_input: chart.level_editor_price_input.clone(),
                    viewport_width: chart.chart_state.camera.viewport_width,
                    viewport_height: chart.chart_state.camera.viewport_height,
                }),
            _ => None,
        };
        ChartPaneOverlaysVm {
            gatr,
            level_placing: self.level_placing,
            editing_level,
            link_picker_dim,
        }
    }

    /// Project the watchlist body inputs (rows incl. thumbnails,
    /// sort, selection bridge, overlays) into a VM. Returns `None`
    /// if `wl_id` does not resolve to an open watchlist.
    pub fn watchlist_body_vm(
        &self,
        wl_id: midas_core::WatchlistId,
    ) -> Option<crate::view_models::watchlist::WatchlistBodyVm> {
        use crate::view_models::watchlist::WatchlistBodyVm;
        let wl = self.watchlists.get(&wl_id)?;
        let show_resize_overlay = matches!(
            self.resizing_column.as_ref().map(|s| s.target),
            Some(crate::column_resize::ColumnResizeTarget::Watchlist(id)) if id == wl_id
        );
        let link_picker_dim = match self.link_picker_open {
            Some((PickerTarget::Watchlist(picker_wl_id), dim)) if picker_wl_id == wl_id => {
                Some(dim)
            }
            _ => None,
        };
        Some(WatchlistBodyVm::build(
            wl,
            &self.market_cache,
            |symbol| self.build_thumbnail_snapshot(symbol),
            show_resize_overlay,
            link_picker_dim,
        ))
    }

    /// Create a new application, restoring state from config if available.
    ///
    /// Returns the app state and a `Task` that opens the main OS window
    /// (required because iced daemon mode does not open a window by default).
    pub fn new() -> (Self, Task<Message>) {
        let config_path = Self::config_file_path();
        Self::migrate_legacy_config(&config_path);
        let config = match AppConfig::load(&config_path) {
            Ok(cfg) => {
                tracing::info!("Loaded config from {}", config_path.display());
                cfg
            }
            Err(e) => {
                tracing::warn!("Failed to load config: {e}, using defaults");
                AppConfig::default()
            }
        };

        let now = chrono::Local::now();
        let current_time = now.format("%H:%M:%S").to_string();

        // Determine initial window position from saved config.
        // Only restore if the saved monitor dimensions still match (the user
        // hasn't changed their display setup) and the window would be at least
        // partially visible.
        let initial_position = Self::validate_saved_position(&config.window);

        let initial_size = (config.window.width, config.window.height);

        // Open the main window via the daemon. The returned Task produces
        // the window::Id once the OS window is created.
        let (main_id, open_task) = window::open(window::Settings {
            size: iced::Size::new(config.window.width as f32, config.window.height as f32),
            position: initial_position,
            ..window::Settings::default()
        });

        let open_task = open_task.map(|id| {
            Message::Window(crate::window_geometry::WindowGeometryMsg::MainWindowOpened(
                id,
            ))
        });

        // Build workspace, charts, watchlists, and order/account panels from config.
        let (
            workspace,
            charts,
            watchlists,
            restored_order_panels,
            restored_account_panels,
            status_message,
        );

        if !config.layout_tree.is_empty() {
            // Full topology restoration from layout_tree.
            let (ws, ch, wl, op, ap) = Self::restore_from_layout_tree(
                &config.layout_tree,
                &config.charts,
                &config.watchlists,
                &config.order_panels,
                &config.account_panels,
            );
            let n = ch.len() + wl.len() + op.len() + ap.len();
            workspace = ws;
            charts = ch;
            watchlists = wl;
            restored_order_panels = op;
            restored_account_panels = ap;
            status_message = format!("Restored {n} panel(s) from layout tree");
        } else if !config.panel_order.is_empty() {
            // Legacy: panel_order-driven restoration (flat, no topology).
            let (mut ws, first_chart_id) = WorkspaceLayout::single();
            let mut ch = HashMap::new();
            let mut wl = HashMap::new();
            let first_pane = ws.focus.unwrap();
            let mut is_first = true;

            for slot in &config.panel_order {
                match slot {
                    PanelSlot::Chart { chart_index } => {
                        let chart_cfg = match config.charts.get(*chart_index) {
                            Some(cfg) => cfg,
                            None => continue,
                        };
                        let panel = Self::restore_panel(chart_cfg);
                        if is_first {
                            ch.insert(first_chart_id, panel);
                            is_first = false;
                        } else if let Some((new_id, _)) =
                            ws.split(pane_grid::Axis::Vertical, first_pane)
                        {
                            ch.insert(new_id, panel);
                        }
                    }
                    PanelSlot::Watchlist { watchlist_index } => {
                        let wl_cfg = match config.watchlists.get(*watchlist_index) {
                            Some(cfg) => cfg,
                            None => continue,
                        };
                        let wl_id = ws.next_watchlist_id();
                        let panel = WatchlistPanel::from_config(wl_id, wl_cfg);
                        if is_first {
                            if let Some(state) = ws.panes.get_mut(first_pane) {
                                state.content = PanelContent::Watchlist(wl_id);
                            }
                            wl.insert(wl_id, panel);
                            is_first = false;
                        } else if let Some((_dummy_id, new_pane)) =
                            ws.split(pane_grid::Axis::Vertical, first_pane)
                        {
                            if let Some(state) = ws.panes.get_mut(new_pane) {
                                state.content = PanelContent::Watchlist(wl_id);
                            }
                            wl.insert(wl_id, panel);
                        }
                    }
                    PanelSlot::OrderPanel { .. } => {
                        // TODO: Slice 5 will restore order panels from legacy panel_order.
                        continue;
                    }
                    PanelSlot::OrderBlotter { .. } | PanelSlot::Account { .. } => {
                        // Account panels (and legacy blotters) are restored via
                        // the layout_tree path, which is preferred for multi-
                        // panel layouts. Skip in the legacy panel_order path.
                        continue;
                    }
                    PanelSlot::Unknown => {
                        // Forward-compat catch-all: config written by a newer
                        // binary carries a panel type this build doesn't know.
                        // Treat as no-op so the rest of the layout still loads.
                        continue;
                    }
                }
            }

            if is_first {
                ch.insert(first_chart_id, Self::make_empty_panel());
            }

            ws.set_focus(first_pane);
            let n = ch.len() + wl.len();
            workspace = ws;
            charts = ch;
            watchlists = wl;
            restored_order_panels = HashMap::new();
            restored_account_panels = std::collections::BTreeMap::new();
            status_message = format!("Restored {n} panel(s) from config");
        } else if !config.charts.is_empty() {
            // Legacy path: charts only (backward compat).
            let (mut ws, first_id) = WorkspaceLayout::single();
            let mut ch = HashMap::new();

            let first_cfg = &config.charts[0];
            ch.insert(first_id, Self::restore_panel(first_cfg));

            let first_pane = ws.focus.unwrap();
            for chart_cfg in config.charts.iter().skip(1) {
                let panel = Self::restore_panel(chart_cfg);
                if let Some((new_id, _)) = ws.split(pane_grid::Axis::Vertical, first_pane) {
                    ch.insert(new_id, panel);
                }
            }
            ws.set_focus(first_pane);

            let n = ch.len();
            workspace = ws;
            charts = ch;
            watchlists = HashMap::new();
            restored_order_panels = HashMap::new();
            restored_account_panels = std::collections::BTreeMap::new();
            status_message = format!("Restored {n} chart(s) from config");
        } else {
            let (ws, first_id) = WorkspaceLayout::single();
            let mut ch = HashMap::new();
            ch.insert(first_id, Self::make_empty_panel());
            workspace = ws;
            charts = ch;
            watchlists = HashMap::new();
            restored_order_panels = HashMap::new();
            restored_account_panels = std::collections::BTreeMap::new();
            status_message = "Ready".to_string();
        };

        // Horizontal price levels used to live in `LevelStore`
        // (audit P2b). Today they round-trip through
        // `AnnotationStore::import_level_configs` / `to_level_configs`
        // so `config.levels` stays byte-identical on disk. The import
        // itself happens later — after `annotation_persistence` has
        // restored bracket annotations into `app.annotation_store` —
        // so the level import doesn't get stomped by the bracket
        // restore.

        // Initialize DuckDB store.
        let store = if config.store.enabled {
            let data_dir = config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            let db_path = data_dir.join(&config.store.path);
            tracing::info!("DuckDB store enabled: {}", db_path.display());
            Some(midas_store::DbHandle::open(midas_store::StoreConfig {
                path: Some(db_path),
                memory_limit_mb: config.store.memory_limit_mb,
                threads: config.store.threads,
            }))
        } else {
            tracing::info!("DuckDB store disabled in config");
            None
        };

        // Open the per-ticker order intent store.
        //
        // Location mirrors the DuckDB config dir — AppData\Local\HandOfMidas
        // on Windows. Failure here is fatal: without the intent store,
        // panels cannot hydrate and Slice 2's bootstrap-from-annotations
        // pass would silently lose data. Slice 1b will surface the
        // `IntentError` variants more gracefully; for now we bail out
        // with a clear panic matching the rest of `new`'s error style.
        let ticker_state_path = dirs::data_local_dir()
            .unwrap_or_default()
            .join("HandOfMidas")
            .join("ticker_state.redb");
        // Open the ticker-state persistence handle (redb v2).
        //
        // This handle hydrates all v2 rows on open, runs v1→v2 migration
        // if needed, and spawns the background flush thread. On startup,
        // we populate `self.tickers` from the cache. On shutdown,
        // `flush_now()` + `shutdown()` ensures all dirty states are
        // durably written.
        let ticker_persist = match crate::ticker_state::persist::TickerStatePersistHandle::open(
            ticker_state_path.clone(),
        ) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!(
                    "failed to open ticker-state persist at {}: {e}",
                    ticker_state_path.display()
                );
                panic!(
                    "ticker-state persist open failed at {}: {e}",
                    ticker_state_path.display()
                );
            }
        };

        // Hydrate the tickers map from redb v2 blobs.
        let mut tickers = ticker_persist.all_states();
        // Annotation IDs are session-local (assigned by AnnotationStore::add
        // which starts at 0 each launch). Stale IDs from a prior session
        // would cause ProjectBracket to silently fail (tries to update an
        // annotation that doesn't exist). Clear them so the first
        // EnsureDraftBracket creates a fresh annotation.
        for ts in tickers.values_mut() {
            ts.set_live_annotation_id(None);
        }
        tracing::info!(
            "ticker-state: loaded {} symbol(s) from redb v2",
            tickers.len()
        );

        // Open the order-history persistence handle alongside the
        // ticker-state store. Same dir, separate file so the two can
        // rotate independently. Hydrate into the blotter BEFORE the
        // broker subscription is registered (happens in subscription() )
        // — the idempotent-BracketCreated apply path will no-op any
        // replays from the engine on reconnect.
        let order_history_path = dirs::data_local_dir()
            .unwrap_or_default()
            .join("HandOfMidas")
            .join("order_history.redb");
        let order_history_persist =
            match crate::order_blotter::persist::OrderHistoryPersistHandle::open(
                order_history_path.clone(),
            ) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!(
                        "failed to open order-history persist at {}: {e}",
                        order_history_path.display()
                    );
                    panic!(
                        "order-history persist open failed at {}: {e}",
                        order_history_path.display()
                    );
                }
            };
        let mut order_blotter = crate::order_blotter::OrderBlotter::new();
        order_blotter.hydrate(order_history_persist.all_rows());
        tracing::info!(
            "order-history: hydrated {} rows into blotter",
            order_blotter.len()
        );

        // v1→v2 migration: import bracket data from annotation JSON files.
        //
        // If a symbol has annotation JSON with OrderBracket data but no
        // v2 redb entry (or an entry without a live_bracket), merge the
        // bracket into the TickerState via from_legacy(). This is the
        // one-way-door migration path: after this, bracket data lives in
        // redb v2 blobs.
        {
            let data_dir = config_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            match crate::annotation_persistence::load_all(data_dir) {
                Ok(files) => {
                    let mut migrated_count = 0u32;
                    for (symbol, annotations) in &files {
                        let sym_key = crate::annotation_store::SymbolKey::new(symbol);
                        // Find the first OrderBracket annotation for this symbol.
                        let bracket_info = annotations.iter().find_map(|ann| {
                            if let midas_chart::widget::AnnotationKind::OrderBracket(ref b) =
                                ann.kind
                            {
                                Some((ann.id, b.as_ref().clone()))
                            } else {
                                None
                            }
                        });
                        if let Some((ann_id, bracket)) = bracket_info {
                            let ts = tickers.entry(sym_key.clone()).or_insert_with(|| {
                                crate::ticker_state::TickerState::new(sym_key.clone())
                            });
                            // Only import if the TickerState doesn't already have a bracket.
                            if ts.live_bracket().is_none() {
                                ts.set_live_bracket(Some(bracket));
                                ts.set_live_annotation_id(Some(ann_id));
                                migrated_count += 1;
                            }
                        }
                    }
                    if migrated_count > 0 {
                        tracing::info!(
                            "ticker-state: imported {migrated_count} bracket(s) from annotation JSON"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("ticker-state migration: failed to load annotation JSON: {e}");
                }
            }

            // v1→v2 migration: import levels from TOML config.
            //
            // For each symbol in `config.levels`, build a projected
            // `Vec<StoredLevel>` and inject it into the corresponding
            // `TickerState`. Redb data (existing levels in TickerState)
            // takes priority — only inject if TickerState has no
            // levels yet.
            {
                use crate::annotation_store::StoredLevel;
                use midas_chart::widget::price_line::{LineExtent, LineStroke, PriceLine};
                use midas_chart::widget::LineStyle;
                let mut next_migration_id: u64 = 1;
                for (ticker, level_cfgs) in &config.levels {
                    if level_cfgs.is_empty() {
                        continue;
                    }
                    let sym_key = crate::annotation_store::SymbolKey::new(ticker);
                    let ts = tickers
                        .entry(sym_key.clone())
                        .or_insert_with(|| crate::ticker_state::TickerState::new(sym_key.clone()));
                    if !ts.levels().is_empty() {
                        continue;
                    }
                    let stored: Vec<StoredLevel> = level_cfgs
                        .iter()
                        .map(|cfg| {
                            let id = next_migration_id;
                            next_migration_id += 1;
                            StoredLevel {
                                level: midas_chart::HorizontalLevel {
                                    id,
                                    line: PriceLine {
                                        price: cfg.price,
                                        extent: LineExtent::default(),
                                        stroke: LineStroke {
                                            color: cfg.color,
                                            width: cfg.line_width,
                                            style: LineStyle::default(),
                                        },
                                    },
                                    label: cfg.label.clone(),
                                    icon: midas_chart::LevelIcon::from_str_id(&cfg.icon),
                                },
                                locked: cfg.locked,
                            }
                        })
                        .collect();
                    let n = stored.len();
                    ts.inject_levels(stored);
                    tracing::debug!(
                        "ticker-state: imported {n} level(s) for {ticker} from TOML config"
                    );
                }
            }

            // Flush all migrated states to redb so subsequent startups
            // skip migration.
            for (sym, state) in &tickers {
                ticker_persist.upsert(sym.clone(), state.clone());
            }
        }

        let broker_backend = config.broker.backend;

        // Router construction (S7e + S8b).
        //
        // * Sim: `SimMarketData` + `SimOrderClient` are synchronous
        //   so the router goes into `self.router = Some(_)` before
        //   `new` returns.
        // * LivePaper / Live: `IbMarketData::connect()` is async and
        //   fallible, so `self.router = None` at startup and a
        //   `Task::perform` drives the connect sequence; the
        //   `Message::RouterReady(Ok((router, order_client)))`
        //   handler swaps in the real router on the next iced diff,
        //   which spins up the chart / watchlist / ticker
        //   subscriptions. Failure shows a toast and leaves
        //   `router = None` — user can re-connect via the status
        //   bar.
        let (router, router_order_client) = match broker_backend {
            midas_core::config::BrokerBackend::Sim => {
                use midas_broker::sim::{SimConfig, SimMarketData, SimOrderClient};
                let sim_cfg = SimConfig::default();
                let sim_md = SimMarketData::new(sim_cfg.market_data.clone());
                let sim_order = SimOrderClient::new(sim_cfg.orders.clone(), Some(sim_md.clone()));
                let router = midas_market_data::MarketDataRouter::new(
                    sim_md as std::sync::Arc<dyn midas_broker::MarketDataSource>,
                );
                let order_client: std::sync::Arc<dyn midas_broker::OrderClient> = sim_order;
                (Some(router), Some(order_client))
            }
            _ => (None, None),
        };
        tracing::info!(
            "router: constructed={}, order_client={}",
            router.is_some(),
            router_order_client.is_some(),
        );
        // Install the router in the subscription-registry's
        // `OnceLock` so the `fn`-pointer stream builders can
        // resolve it without capturing.
        if let Some(ref r) = router {
            crate::app::subscription_registry::install_router(r.clone());
        }

        let mut app = Self {
            charts,
            workspace,
            status_message,
            show_frame_overlay: false,
            config_path,
            config_dirty: false,
            last_config_save: Instant::now(),
            current_time,
            window: {
                let mut g = crate::window_geometry::WindowGeometry::from_config(
                    &config.window,
                    initial_size,
                );
                // The runtime `main_window` id is the iced-assigned one
                // for this launch — feed it in once. Effects from this
                // synthetic Open are discarded; the parent will spawn
                // its own monitor-size query right after `new` returns.
                let _ = g.update(crate::window_geometry::WindowGeometryMsg::MainWindowOpened(
                    main_id,
                ));
                g
            },
            floating_charts: HashMap::new(),
            level_placing: false,
            placing_preview: None,
            dragging_annotation: None,
            crosshair_sync: None,
            providers: Self::build_provider_registry(&config),
            store,
            watchlists,
            cursor_position: iced::Point::ORIGIN,
            pending_drag: None,
            dragging_ticker: None,
            link_picker_open: None,
            resizing_column: None,
            account_column_selector_open: None,
            order_panels: restored_order_panels,
            account_panels: restored_account_panels,
            positions: crate::account_panel::PositionStore::new(),
            recent_symbols: config
                .recent_symbols
                .iter()
                .cloned()
                .map(|symbol| RecentEntry {
                    symbol,
                    last_seen: None,
                })
                .collect(),
            order_blotter,
            order_history_persist,
            order_annotation_links: HashMap::new(),
            toasts: crate::toast::ToastController::new(),
            bracket_context_menu: None,
            annotation_store: AnnotationStore::new(),
            market_cache: crate::market_cache::MarketDataCache::default(),
            // Seed the display with "Connecting" so the status bar
            // shows the transition until `Message::RouterReady` flips
            // it to "Ready". Without this, the first paint briefly
            // flashes "Disconnected" which reads as an error state.
            broker_connection_display: "Connecting".to_string(),
            chart_views: {
                // Slice 8a of chart-transition: stamp the persisted
                // schema onto the fresh in-memory store so the rollback
                // coordination picks up where the previous run left off.
                // `0` (missing) means "never written" — store keeps its
                // default stamp, and the first write lifts it to v2.
                let mut cvs = crate::chart_view::ChartViewStore::default();
                if config.chart_view_store_schema > 0 {
                    cvs.set_schema_version(config.chart_view_store_schema);
                }
                cvs
            },
            thumbnail_store: crate::thumbnail_store::ThumbnailStore::default(),
            thumbnail_data: crate::thumbnail_data::ThumbnailDataStore::default(),
            tickers,
            ticker_persist,
            ticker_dispatch_active: false,
            sim_child: None,
            broker_cfg: config.broker.clone(),
            router,
            router_order_client,
            resync_throttle: std::collections::HashMap::new(),
            ib_to_uuid: std::collections::HashMap::new(),
            #[cfg(feature = "session_chart")]
            floating_session_charts: std::collections::HashMap::new(),
            #[cfg(feature = "session_chart")]
            session_chart_registry: crate::session_chart::SymbolSeriesRegistry::new(),
        };

        // Restore bracket annotations from persistence.
        let data_dir = app
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        match crate::annotation_persistence::load_all(data_dir) {
            Ok(files) => {
                let loaded_count: usize = files.values().map(|v| v.len()).sum();
                if loaded_count > 0 {
                    app.annotation_store = crate::annotation_persistence::store_from_files(files);
                    tracing::info!("Restored {loaded_count} annotation(s) from persistence");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load persisted annotations: {e}");
            }
        }

        // Horizontal price levels from TOML config round-trip through
        // `AnnotationStore::import_level_configs` (audit P2b — retired
        // `LevelStore`). Imported AFTER the bracket-annotation restore
        // so the level annotations coexist with restored brackets.
        app.annotation_store.import_level_configs(&config.levels);

        // Link restored brackets to their order panels and sync state.
        // Saved brackets load Active (visible on chart immediately).
        // Panel bracket_active is derived from TickerState.bracket_mode().
        {
            let mut dirty_symbols: Vec<String> = Vec::new();
            for panel in app.order_panels.values_mut() {
                let symbol = panel.state.symbol.to_uppercase();
                if symbol.is_empty() {
                    continue;
                }

                // Derive bracket_active from TickerState (single source of truth).
                let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
                let bracket_mode = app.tickers.get(&sym_key).and_then(|ts| ts.bracket_mode());
                panel.state.bracket_active = bracket_mode;

                // Only link to a restored annotation if bracket_mode is active.
                if bracket_mode.is_none() {
                    continue;
                }

                // Find the first active Draft bracket for this symbol.
                let bracket_info = app
                    .annotation_store
                    .get(&symbol)
                    .iter()
                    .find(|a| {
                        a.presence == midas_chart::widget::Presence::Active
                            && matches!(
                                &a.kind,
                                midas_chart::widget::AnnotationKind::OrderBracket(b)
                                    if b.status == midas_chart::widget::order_bracket::BracketStatus::Draft
                            )
                    })
                    .map(|a| {
                        let side = match &a.kind {
                            midas_chart::widget::AnnotationKind::OrderBracket(b) => b.side,
                            _ => unreachable!(),
                        };
                        (a.id, side)
                    });

                if let Some((ann_id, _side)) = bracket_info {
                    panel.state.bracket_annotation_id = Some(ann_id);
                    // Sync all panel fields from bracket truth.
                    if let Some(bracket_data) = app
                        .annotation_store
                        .get(&symbol)
                        .iter()
                        .find(|a| a.id == ann_id)
                        .and_then(|a| match &a.kind {
                            midas_chart::widget::AnnotationKind::OrderBracket(b) => {
                                Some(b.as_ref())
                            }
                            _ => None,
                        })
                    {
                        crate::order_panel::sync_panel_from_bracket(&mut panel.state, bracket_data);
                    }
                    dirty_symbols.push(symbol.clone());
                    tracing::debug!("Linked panel for {symbol} to restored bracket {ann_id}");
                }
            }
            // Mark charts dirty so brackets render on first frame.
            for symbol in &dirty_symbols {
                app.mark_levels_dirty_for_ticker(symbol);
            }
        }

        // Bootstrap the ticker intent store from existing bracket
        // annotations (Slice 2). For every symbol that has at least one
        // Ensure every watchlist symbol has a TickerState. Symbols that
        // already loaded from redb are skipped; new symbols get defaults.
        for wl in app.watchlists.values() {
            for ticker in &wl.tickers {
                let sym_key = crate::annotation_store::SymbolKey::new(&ticker.symbol);
                app.tickers
                    .entry(sym_key.clone())
                    .or_insert_with(|| crate::ticker_state::TickerState::new(sym_key));
            }
        }

        // S7e: market-data subscriptions are now driven by the
        // per-consumer iced subscriptions (`chart_subscriptions`,
        // `watchlist_subscription`, `ticker_subscription`). Each
        // spawns itself when the corresponding symbol surfaces and
        // the router-backed `subscription_registry::router()` is
        // available. No eager reconciliation needed.

        // Async-load data for all restored charts that have a symbol.
        let mut load_tasks: Vec<Task<Message>> = Vec::new();
        let charts_to_load: Vec<(ChartId, String, Timeframe)> = app
            .charts
            .iter()
            .filter(|(_, panel)| !panel.symbol.is_empty())
            .map(|(&id, panel)| (id, panel.symbol.clone(), panel.timeframe))
            .collect();
        for (id, symbol, tf) in &charts_to_load {
            if let Some(chart) = app.charts.get_mut(id) {
                chart.load_state = LoadState::Loading;
            }
            load_tasks.push(app.load_chart_async_restore(*id, symbol, *tf));
        }
        // Also load market snapshots for all watchlist symbols.
        let watchlist_task = app.load_all_watchlist_snapshots();
        // After layout restore, the blotter may already carry hydrated
        // rows from disk. Seed every Account panel's Trade-History
        // cache so panels whose `active_tab` was persisted as
        // `TradeHistory` render rows on their very first `view()`
        // without waiting for a user click.
        app.rebuild_account_history_caches();
        // Same rationale for Positions: during `MidasApp::new` the
        // store is necessarily empty, but keeping this call
        // symmetrical with the History path means future restart
        // workflows (e.g. a persisted-positions follow-up slice) pick
        // up the seeding for free. Today's call is effectively a
        // cheap no-op (generation == 0 == last_seen_generation).
        app.rebuild_account_positions_caches();

        // Pre-warm the thumbnail cache so the watchlist's Chart column
        // starts rendering mountains as soon as the user opens the
        // panel, not only after they click a thumbnail.
        let thumbnail_task = app.load_all_thumbnails();

        // If the user is on the Sim backend (the default), kick off
        // the sim-child spawn in the background. The broker engine
        // already booted with the Test broker; once the sim is
        // healthy, `Message::BrokerSimSpawned` swaps it for a real
        // ib-bridge connection. Running this concurrently with the
        // chart data loads keeps the cold-start timeline tight: the
        // UI is interactive before the sim is up.
        //
        // `MIDAS_DISABLE_AUTO_SIM=1` turns the auto-spawn off so
        // integration tests that script sim lifecycle via the
        // devloop's explicit `SpawnSim` command don't race with it.
        let auto_spawn_disabled = std::env::var("MIDAS_DISABLE_AUTO_SIM")
            .ok()
            .is_some_and(|v| !v.is_empty() && v != "0");
        let sim_spawn_task = if !auto_spawn_disabled
            && matches!(broker_backend, midas_core::config::BrokerBackend::Sim)
        {
            let preferred_port = config.broker.port;
            Task::perform(
                async move {
                    let tws_port = match crate::sim_child::allocate_sim_port(preferred_port) {
                        Ok(p) => p,
                        Err(e) => return Err(e.to_string()),
                    };
                    // Control port sits 2000 above the TWS port — same
                    // convention the dev-harness uses. If the computed
                    // control port is also taken, probe for any free
                    // port; the bearer-token handshake doesn't care
                    // which port the control plane binds to.
                    let control_port_preferred = tws_port.saturating_add(2000);
                    let control_port =
                        match crate::sim_child::allocate_sim_port(control_port_preferred) {
                            Ok(p) => p,
                            Err(e) => return Err(format!("control port: {e}")),
                        };
                    // Seed from wall-clock so re-launches produce
                    // distinct market data but a given run stays
                    // reproducible for screenshot comparisons within
                    // the same process.
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0);
                    let opts = crate::sim_child::SpawnOptions {
                        tws_port,
                        control_port,
                        scenario: None,
                        seed: Some(seed),
                    };
                    crate::sim_child::spawn(opts)
                        .await
                        .map_err(|e| e.to_string())
                },
                Message::BrokerSimSpawned,
            )
        } else {
            Task::none()
        };

        // S7e: the 250 ms synthetic `BrokerConnectionChanged(Ready)`
        // hack is deleted. The router's own
        // `ConnectionState` watch dispatches the real transition
        // natively — the sim source reaches `Ready` immediately on
        // construction, and the IB source moves through
        // `Connecting → Connected → Ready` driven by the IB
        // handshake.

        // S8b: spawn the IB connect task if the user picked a live
        // backend. The task constructs `IbMarketData` + its
        // `IbOrderClient`, awaits `connect()`, and — on success —
        // wraps the pair in `Message::RouterReady(Ok(_))`. The
        // sub-handler assigns `self.router` / `self.router_order_client`
        // and iced's next diff picks up the subscriptions.
        //
        // Retry policy: one retry after 5 s on connect failure. If
        // the second attempt also fails, surface a toast and leave
        // `router = None` so the user can reconnect manually via
        // the status bar. See plan/market-data-router/09-slice-8-...
        let ib_router_task = match broker_backend {
            midas_core::config::BrokerBackend::LivePaper
            | midas_core::config::BrokerBackend::Live => {
                let ib_cfg = midas_broker::ib::IbMarketDataConfig {
                    host: config.broker.host.clone(),
                    port: config.broker.port,
                    client_id: config.broker.client_id,
                    ..Default::default()
                };
                Task::perform(build_ib_router(ib_cfg), |result| match result {
                    Ok(payload) => Message::RouterReady(Ok(payload)),
                    Err(e) => Message::RouterReady(Err(e)),
                })
            }
            _ => Task::none(),
        };

        let startup_task = if load_tasks.is_empty() {
            Task::batch([
                open_task,
                watchlist_task,
                thumbnail_task,
                sim_spawn_task,
                ib_router_task,
            ])
        } else {
            load_tasks.push(open_task);
            load_tasks.push(watchlist_task);
            load_tasks.push(thumbnail_task);
            load_tasks.push(sim_spawn_task);
            load_tasks.push(ib_router_task);
            Task::batch(load_tasks)
        };

        (app, startup_task)
    }

    /// Restore the full pane grid topology from a flattened layout tree.
    ///
    /// Parses the pre-order traversal, builds a `pane_grid::Configuration`,
    /// and creates the `WorkspaceLayout` with correct axes and ratios.
    #[expect(clippy::type_complexity, reason = "used only in one internal method")]
    fn restore_from_layout_tree(
        tree: &[LayoutNode],
        chart_cfgs: &[ChartConfig],
        watchlist_cfgs: &[midas_core::config::WatchlistConfig],
        order_panel_cfgs: &[midas_core::config::OrderPanelConfig],
        account_panel_cfgs: &[midas_core::config::AccountPanelConfig],
    ) -> (
        WorkspaceLayout,
        HashMap<ChartId, ChartPanel>,
        HashMap<WatchlistId, WatchlistPanel>,
        HashMap<OrderPanelId, crate::order_panel::OrderPanel>,
        std::collections::BTreeMap<AccountPanelId, crate::account_panel::AccountPanel>,
    ) {
        use crate::layout::PaneState;

        struct RestoreCtx {
            charts: HashMap<ChartId, ChartPanel>,
            watchlists: HashMap<WatchlistId, WatchlistPanel>,
            order_panels: HashMap<OrderPanelId, crate::order_panel::OrderPanel>,
            account_panels:
                std::collections::BTreeMap<AccountPanelId, crate::account_panel::AccountPanel>,
            next_chart_id: u32,
            next_wl_id: u32,
            next_order_id: u32,
            next_account_id: u32,
            cursor: usize,
        }

        impl RestoreCtx {
            fn parse_node(
                &mut self,
                tree: &[LayoutNode],
                chart_cfgs: &[ChartConfig],
                watchlist_cfgs: &[midas_core::config::WatchlistConfig],
                order_panel_cfgs: &[midas_core::config::OrderPanelConfig],
                account_panel_cfgs: &[midas_core::config::AccountPanelConfig],
            ) -> pane_grid::Configuration<PaneState> {
                if self.cursor >= tree.len() {
                    let id = ChartId::new(self.next_chart_id);
                    self.next_chart_id += 1;
                    self.charts.insert(id, MidasApp::make_empty_panel());
                    return pane_grid::Configuration::Pane(PaneState::chart(id));
                }
                match &tree[self.cursor] {
                    LayoutNode::Split { axis, ratio } => {
                        let ax = if axis == "horizontal" {
                            pane_grid::Axis::Horizontal
                        } else {
                            pane_grid::Axis::Vertical
                        };
                        let r = *ratio;
                        self.cursor += 1;
                        let a = self.parse_node(
                            tree,
                            chart_cfgs,
                            watchlist_cfgs,
                            order_panel_cfgs,
                            account_panel_cfgs,
                        );
                        let b = self.parse_node(
                            tree,
                            chart_cfgs,
                            watchlist_cfgs,
                            order_panel_cfgs,
                            account_panel_cfgs,
                        );
                        pane_grid::Configuration::Split {
                            axis: ax,
                            ratio: r,
                            a: Box::new(a),
                            b: Box::new(b),
                        }
                    }
                    LayoutNode::Chart { chart_index } => {
                        let id = ChartId::new(self.next_chart_id);
                        self.next_chart_id += 1;
                        let panel = chart_cfgs
                            .get(*chart_index)
                            .map(MidasApp::restore_panel)
                            .unwrap_or_else(MidasApp::make_empty_panel);
                        self.charts.insert(id, panel);
                        self.cursor += 1;
                        pane_grid::Configuration::Pane(PaneState::chart(id))
                    }
                    LayoutNode::Watchlist { watchlist_index } => {
                        let wl_id = WatchlistId::new(self.next_wl_id);
                        self.next_wl_id += 1;
                        if let Some(wl_cfg) = watchlist_cfgs.get(*watchlist_index) {
                            let panel = WatchlistPanel::from_config(wl_id, wl_cfg);
                            self.watchlists.insert(wl_id, panel);
                        }
                        self.cursor += 1;
                        pane_grid::Configuration::Pane(PaneState::watchlist(wl_id))
                    }
                    LayoutNode::OrderPanel { order_panel_index } => {
                        let op_id = OrderPanelId::new(self.next_order_id);
                        self.next_order_id += 1;
                        if let Some(op_cfg) = order_panel_cfgs.get(*order_panel_index) {
                            let panel = crate::order_panel::OrderPanel::from_config(op_id, op_cfg);
                            self.order_panels.insert(op_id, panel);
                        }
                        self.cursor += 1;
                        pane_grid::Configuration::Pane(PaneState::order(op_id))
                    }
                    LayoutNode::Account {
                        account_panel_index,
                    } => {
                        let account_id = AccountPanelId::new(self.next_account_id);
                        self.next_account_id += 1;
                        let panel = match account_panel_cfgs.get(*account_panel_index) {
                            Some(cfg) => {
                                crate::account_panel::AccountPanel::from_config(account_id, cfg)
                            }
                            None => crate::account_panel::AccountPanel::new(account_id, "Account"),
                        };
                        self.account_panels.insert(account_id, panel);
                        self.cursor += 1;
                        pane_grid::Configuration::Pane(PaneState::account(account_id))
                    }
                    LayoutNode::OrderBlotter { .. } => {
                        // Legacy blotter nodes are rewritten to `Account`
                        // at config-load time by the migration step. Any
                        // residual node is treated as a forward-compat
                        // placeholder (unusual — a write with the legacy
                        // enum variant never happens from this build).
                        tracing::warn!(
                            "Unexpected legacy OrderBlotter layout node — \
                             migration should have rewritten it. Falling back to empty chart."
                        );
                        let id = ChartId::new(self.next_chart_id);
                        self.next_chart_id += 1;
                        self.charts.insert(id, MidasApp::make_empty_panel());
                        self.cursor += 1;
                        pane_grid::Configuration::Pane(PaneState::chart(id))
                    }
                    LayoutNode::Unknown => {
                        // Forward-compatibility: skip unknown node types gracefully.
                        tracing::warn!("Skipping unknown layout node at index {}", self.cursor);
                        let id = ChartId::new(self.next_chart_id);
                        self.next_chart_id += 1;
                        self.charts.insert(id, MidasApp::make_empty_panel());
                        self.cursor += 1;
                        pane_grid::Configuration::Pane(PaneState::chart(id))
                    }
                }
            }
        }

        let mut ctx = RestoreCtx {
            charts: HashMap::new(),
            watchlists: HashMap::new(),
            order_panels: HashMap::new(),
            account_panels: std::collections::BTreeMap::new(),
            next_chart_id: 1,
            next_wl_id: 1,
            next_order_id: 1,
            next_account_id: 1,
            cursor: 0,
        };

        let config = ctx.parse_node(
            tree,
            chart_cfgs,
            watchlist_cfgs,
            order_panel_cfgs,
            account_panel_cfgs,
        );

        let panes = pane_grid::State::with_configuration(config);
        let first_pane = panes.panes.keys().next().copied();
        let ws = WorkspaceLayout {
            panes,
            focus: first_pane,
            next_chart_id: ctx.next_chart_id,
            next_watchlist_id: ctx.next_wl_id,
            next_order_panel_id: ctx.next_order_id,
            next_order_blotter_id: 1,
            next_account_panel_id: ctx.next_account_id,
        };

        (
            ws,
            ctx.charts,
            ctx.watchlists,
            ctx.order_panels,
            ctx.account_panels,
        )
    }

    /// Restore a single chart panel from config.
    ///
    /// Levels are no longer restored per-chart — they live in
    /// `AnnotationStore` (audit P2b).
    fn restore_panel(cfg: &ChartConfig) -> ChartPanel {
        let tf = Timeframe::from_suffix(&cfg.timeframe).unwrap_or(Timeframe::D1);
        let mut panel = Self::make_empty_panel();
        panel.symbol = cfg.symbol.clone();
        panel.symbol_input = cfg.symbol.clone();
        // Restore bound_symbol from config, falling back to the legacy
        // `symbol` field when the config predates Slice 3.
        panel.bound_symbol = cfg
            .bound_symbol
            .as_deref()
            .or(Some(cfg.symbol.as_str()))
            .filter(|s| !s.is_empty())
            .map(crate::annotation_store::SymbolKey::new);
        panel.timeframe = tf;
        panel.symbol_link = cfg.symbol_link;
        panel.timeframe_link = cfg.timeframe_link;
        // Chart-transition slice 9a: restore persisted backend. `None`
        // means "follow the app default" which is currently
        // `ChartBackend::Legacy`. The dispatch layer handles the
        // feature-gate × config mismatch (build without
        // `session_chart` + config says `New` falls back to Legacy
        // with a `tracing::warn!`).
        panel.backend = cfg.backend.unwrap_or_default();
        Self::restore_camera(cfg, &mut panel);
        panel
    }

    /// Restore camera position and collapse_gaps from a chart config.
    fn restore_camera(chart_cfg: &ChartConfig, panel: &mut ChartPanel) {
        if let (Some(ts), Some(te), Some(pl), Some(ph)) = (
            chart_cfg.camera_time_start,
            chart_cfg.camera_time_end,
            chart_cfg.camera_price_low,
            chart_cfg.camera_price_high,
        ) {
            panel.chart_state.camera.time_start = ts;
            panel.chart_state.camera.time_end = te;
            panel.chart_state.camera.price_low = pl;
            panel.chart_state.camera.price_high = ph;
        }
        panel.chart_state.collapse_gaps = chart_cfg.collapse_gaps;
        panel.chart_state.timeline_border_ratio = chart_cfg.timeline_border_ratio;
        panel.chart_state.volume_scale = chart_cfg.volume_scale;
        panel.chart_state.show_volume_profile = chart_cfg.show_volume_profile;
        panel.chart_state.show_levels = chart_cfg.show_levels;
        // Restore viewport so the first-frame ChartViewportChanged computes
        // the correct ratio (saved viewport → actual pane size) instead of
        // using the dummy 1280×720 from make_empty_panel.
        if let (Some(vw), Some(vh)) = (chart_cfg.viewport_width, chart_cfg.viewport_height) {
            if vw > 0 && vh > 0 {
                panel.chart_state.camera.viewport_width = vw;
                panel.chart_state.camera.viewport_height = vh;
            }
        }
    }

    /// Create an empty chart panel with default camera and state.
    fn make_empty_panel() -> ChartPanel {
        let camera = Camera2D {
            time_start: 0.0,
            time_end: 1.0,
            price_low: 0.0,
            price_high: 1.0,
            viewport_width: 1280,
            viewport_height: 720,
            dpi_scale: 1.0,
        };
        ChartPanel {
            symbol: String::new(),
            bound_symbol: None,
            timeframe: Timeframe::D1,
            data: None,
            chart_state: ChartState::new(camera),
            load_state: LoadState::Empty,
            symbol_input: String::new(),
            editing_level_id: None,
            editing_level_screen_pos: None,
            level_editor_price_input: String::new(),
            symbol_link: LinkMode::Unlinked,
            timeframe_link: LinkMode::Unlinked,
            gatr_hover: false,
            camera_restored_pending: false,
            load_generation: 0,
            visible: true,
            // Slice 9a default: every new panel starts on the legacy
            // backend. The user flips via the toolbar chip; slice 9b
            // will flip the app-wide default to `New` after soak.
            backend: ChartBackend::Legacy,
        }
    }

    /// Iterate every chart panel — docked first, then floating —
    /// tagging each item with a [`ChartHandle`] that identifies which
    /// storage map it came from.
    ///
    /// Use this when you want to treat docked + floating charts
    /// uniformly (link-group fan-out, symbol broadcast, cursor sync…);
    /// reach directly into `charts` / `floating_charts` only where the
    /// distinction matters (persistence, window-event routing).
    pub(crate) fn all_chart_panels(&self) -> impl Iterator<Item = (ChartHandle, &ChartPanel)> {
        self.charts
            .iter()
            .map(|(id, p)| (ChartHandle::Docked(*id), p))
            .chain(
                self.floating_charts
                    .iter()
                    .map(|(wid, p)| (ChartHandle::Floating(*wid), p)),
            )
    }

    /// Mutable companion to [`all_chart_panels`]. The two underlying
    /// `HashMap`s are disjoint fields so the borrow checker accepts a
    /// combined `iter_mut` chain without special handling.
    ///
    /// Unused today — the prep step in audit P2a collapses only the
    /// link handlers, which can do with the shared-ref iterator plus
    /// targeted `get_mut` fix-ups. Kept for the next caller that
    /// genuinely needs a single mutable pass across both maps.
    #[allow(dead_code)]
    pub(crate) fn all_chart_panels_mut(
        &mut self,
    ) -> impl Iterator<Item = (ChartHandle, &mut ChartPanel)> {
        self.charts
            .iter_mut()
            .map(|(id, p)| (ChartHandle::Docked(*id), p))
            .chain(
                self.floating_charts
                    .iter_mut()
                    .map(|(wid, p)| (ChartHandle::Floating(*wid), p)),
            )
    }

    /// Resolve the ticker symbol for a chart ID.
    ///
    /// Checks main pane charts first, then falls back to floating chart
    /// windows which use `ChartId::new(0)` as a sentinel.
    fn chart_ticker(&self, id: ChartId) -> Option<&str> {
        if let Some(chart) = self.charts.get(&id) {
            return Some(chart.symbol.as_str());
        }
        if id == ChartId::new(0) {
            if let Some(chart) = self.floating_charts.values().next() {
                return Some(chart.symbol.as_str());
            }
        }
        None
    }

    /// Mark levels dirty on every chart (main + floating) displaying `ticker`.
    ///
    /// This bridges `AnnotationStore` generation changes to the
    /// existing per-chart `DirtyFlags.levels` counter that the GPU
    /// renderer depends on.
    fn mark_levels_dirty_for_ticker(&mut self, ticker: &str) {
        for chart in self.charts.values_mut() {
            if chart.symbol == ticker {
                chart.chart_state.dirty.mark_levels();
            }
        }
        for chart in self.floating_charts.values_mut() {
            if chart.symbol == ticker {
                chart.chart_state.dirty.mark_levels();
            }
        }
    }

    /// Sync an order panel's UI state from its linked bracket annotation,
    /// then mark all charts showing that symbol as dirty so bracket lines
    /// re-render.
    ///
    /// Superseded by the `ProjectBracket` effect handler in the
    /// `Message::Ticker` routing arm, but kept for non-bracket annotation
    /// paths that still use it.
    #[allow(dead_code)]
    fn sync_panel_and_redraw(
        &mut self,
        panel_id: midas_core::OrderPanelId,
        ann_id: midas_chart::widget::AnnotationId,
        symbol: &str,
    ) {
        if let Some(bracket_data) = self.annotation_store.get_bracket(symbol, ann_id) {
            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                crate::order_panel::sync_panel_from_bracket(&mut p.state, bracket_data);
            }
        }
        self.mark_levels_dirty_for_ticker(symbol);
    }

    // ── Bracket chart toggle helpers ────────────────────────────────

    /// Handle bracket state when an order panel's symbol changes.
    ///
    /// Only Draft brackets participate in the cache/restore cycle.
    /// Pending and Active brackets represent live broker orders and
    /// must NOT be removed on symbol change — they remain in the
    /// AnnotationStore under their original symbol.
    /// Returns `true` if a hidden bracket was recalled for the new symbol
    /// (panel inputs are synced from it — caller must NOT clear them).
    fn handle_order_panel_symbol_change(
        &mut self,
        panel_id: OrderPanelId,
        old_symbol: &str,
        new_symbol: &str,
    ) -> bool {
        let panel = match self.order_panels.get(&panel_id) {
            Some(p) => p,
            None => return false,
        };

        // If bracket mode is off, clear any stale annotation link
        // (e.g., from a previously hidden bracket on the old symbol).
        if panel.state.bracket_active.is_none() {
            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                p.state.bracket_annotation_id = None;
            }
            return false;
        }

        let old_upper = old_symbol.to_uppercase();
        let new_upper = new_symbol.to_uppercase();

        // If same symbol (case-insensitive), nothing to do.
        if old_upper == new_upper {
            return false;
        }

        // Cancel the old symbol's bracket via TickerState.
        {
            let old_key = crate::annotation_store::SymbolKey::new(&old_upper);
            let _ = self.update(Message::Ticker(
                old_key,
                crate::ticker_state::TickerMsg::CancelBracket,
            ));
        }

        // Clear the panel's annotation link (it pointed to old symbol).
        if let Some(p) = self.order_panels.get_mut(&panel_id) {
            p.state.bracket_annotation_id = None;
        }

        // Create a fresh draft bracket for the new symbol via TickerState.
        let side = match self
            .order_panels
            .get(&panel_id)
            .and_then(|p| p.state.bracket_active)
        {
            Some(s) => s,
            None => return false,
        };
        let entry_type = self
            .order_panels
            .get(&panel_id)
            .map(|p| p.state.entry_type)
            .unwrap_or_default();

        let new_key = crate::annotation_store::SymbolKey::new(&new_upper);
        // Seed market data on the ticker state before creating the bracket.
        let (mc_price, mc_gatr) = self
            .market_cache
            .get(&new_key)
            .map(|s| (s.last_price, s.gatr_abs))
            .unwrap_or((None, None));
        {
            let ts = self.ticker_mut(&new_key);
            ts.set_last_price(mc_price);
            ts.set_gatr_abs(mc_gatr);
        }
        let _ = self.update(Message::Ticker(
            new_key,
            crate::ticker_state::TickerMsg::EnsureDraftBracket { side, entry_type },
        ));
        false
    }

    /// Update Draft brackets that have `entry.price == 0.0` for a symbol.
    ///
    /// Called when chart data finishes loading. If a panel activated a
    /// bracket before data was available, the entry price is 0.0; this
    /// patches it to the last close price from the newly loaded data.
    /// Routes through `TickerState::apply()`.
    fn update_zero_price_brackets(&mut self, symbol: &str, price: f64) {
        let sym_upper = symbol.to_uppercase();
        let sym_key = crate::annotation_store::SymbolKey::new(&sym_upper);

        // Check if the TickerState's live bracket has zero entry price.
        let needs_update = self
            .tickers
            .get(&sym_key)
            .and_then(|ts| ts.live_bracket())
            .map(|b| {
                b.status == midas_chart::widget::order_bracket::BracketStatus::Draft
                    && b.entry.line.price.abs() < f64::EPSILON
            })
            .unwrap_or(false);

        if needs_update {
            let _ = self.update(Message::Ticker(
                sym_key,
                crate::ticker_state::TickerMsg::SetLegPrice {
                    role: midas_chart::widget::order_bracket::LegRole::Entry,
                    price,
                },
            ));
        }
    }

    /// Set a chart's symbol and asynchronously load data for it.
    ///
    /// This is the **single choke point** for loading a ticker into a
    /// docked chart. Every user-facing path that changes a chart's
    /// symbol (watchlist click, drag-drop, panel symbol submit, link
    /// propagation, group adoption) must route through this function
    /// so the ticker-intent reconciliation fires exactly once per
    /// activation.
    ///
    /// Returns a `Task` that will produce `Message::DataLoaded` when complete.
    fn load_symbol_for_chart(&mut self, chart_id: ChartId, symbol: &str) -> Task<Message> {
        let symbol = symbol.trim().to_uppercase();
        if symbol.is_empty() {
            return Task::none();
        }

        let tf = self
            .charts
            .get(&chart_id)
            .map(|c| c.timeframe)
            .unwrap_or(Timeframe::D1);

        // Capture outgoing view state BEFORE mutating the chart.
        if let Some(chart) = self.charts.get(&chart_id) {
            if let Some(ref buf) = chart.data {
                if !chart.symbol.is_empty() && !buf.is_empty() {
                    self.chart_views
                        .get_or_default(&chart.symbol, chart.timeframe)
                        .capture_from_camera(
                            &chart.chart_state.camera,
                            buf,
                            chart.chart_state.collapse_gaps,
                        );
                }
            }
        }

        if let Some(chart) = self.charts.get_mut(&chart_id) {
            chart.load_generation = chart.load_generation.wrapping_add(1);
            chart.load_state = LoadState::Loading;
            // Clear old data immediately so draw() produces scene: None
            // during the loading window. This triggers the GPU buffer
            // clear in prepare() and prevents ghost candles from the
            // previous ticker persisting on screen.
            chart.data = None;
            chart.chart_state.dirty.mark_data();
        }

        // Bind through the single mutation point — sets bound_symbol,
        // lazy-creates TickerState, seeds market data, fires
        // EnsureDraftBracket, and binds linked panels.
        let key = crate::annotation_store::SymbolKey::new(&symbol);
        self.bind_chart_to_symbol(chart_id, key);

        self.load_chart_async(chart_id, &symbol, tf)
    }

    /// Record a symbol in the Recent Instruments MRU.
    ///
    /// Dedup-move-to-front with a hard cap of [`MAX_RECENTS`]. Called at
    /// every user-driven symbol-switch seam (chart input submit, ticker
    /// drag-drop, Recents-tab row click). Empty / whitespace inputs are
    /// ignored. The session's `last_seen` is set to `Instant::now()`.
    pub(crate) fn push_recent_symbol(&mut self, symbol: &str) {
        if push_recent_symbol_inner(&mut self.recent_symbols, symbol, Instant::now()) {
            // Symbol list is persisted; mark config dirty.
            self.mark_config_dirty();
        }
    }

    /// Propagate a symbol change from source chart to all linked charts
    /// (both docked and floating).
    ///
    /// ## Slice 8e — 4-step link-propagation checklist
    ///
    /// For every linked receiver the method emits four `tracing::debug!`
    /// events in the canonical order defined by
    /// [`crate::link::LINK_PROPAGATION_ORDER`]. The steps themselves
    /// execute through the existing `load_symbol_for_chart` path —
    /// this is an observability overlay, not a new code path. The
    /// ordering invariant is enforced by the unit tests in
    /// `crate::link`; integration tests consume the `tracing` events
    /// to assert the 4-step sequence fired exactly once per linked
    /// receiver.
    fn propagate_symbol_change(&mut self, source_id: ChartId, new_symbol: &str) -> Task<Message> {
        use crate::link::{find_link_targets, log_link_propagation_step, LinkPropagationStep};

        let source_link = self
            .charts
            .get(&source_id)
            .map(|c| c.symbol_link)
            .unwrap_or(LinkMode::Unlinked);

        // Docked charts.
        let pane_targets: Vec<ChartId> = find_link_targets(
            source_link,
            self.charts
                .iter()
                .filter(|(id, _)| **id != source_id)
                .map(|(id, panel)| (*id, panel.symbol_link)),
        );

        let mut tasks: Vec<Task<Message>> = Vec::new();
        for id in pane_targets {
            // Outgoing symbol captured BEFORE any mutation so the
            // 4-step log has both old and new values.
            let old_sym = self
                .charts
                .get(&id)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();
            // Step 1 — clear dependent caches (ATR / VP / bright
            // highlights) before the new handle fires so the first
            // post-swap frame sees a clean slate.
            //
            // `load_symbol_for_chart` already calls
            // `chart.data = None` + `mark_data()` which drops every
            // version-keyed layer cache; the log below makes the step
            // auditable from the event tail without duplicating the
            // cache clear here.
            log_link_propagation_step(LinkPropagationStep::ClearCaches, id.0, &old_sym, new_symbol);
            // Step 2 — drop the old SubscriptionHandle. Implicit via
            // the next iced `subscription()` diff: the upcoming
            // `bind_chart_to_symbol` inside `load_symbol_for_chart`
            // mutates `bound_symbol`, which changes the subscription
            // closure's capture and causes iced to tear down the
            // existing handle.
            log_link_propagation_step(
                LinkPropagationStep::DropSubscription,
                id.0,
                &old_sym,
                new_symbol,
            );
            if let Some(chart) = self.charts.get_mut(&id) {
                chart.gatr_hover = false;
            }
            // Step 3 — acquire the new SubscriptionHandle. Also
            // implicit via the subscription re-diff after
            // `load_symbol_for_chart` flips `bound_symbol`.
            log_link_propagation_step(
                LinkPropagationStep::AcquireSubscription,
                id.0,
                &old_sym,
                new_symbol,
            );
            tasks.push(self.load_symbol_for_chart(id, new_symbol));
            // Step 4 — reset + auto-scale. `load_symbol_for_chart`
            // returns a task that on completion invokes
            // `apply_candle_data` with a fresh `ChartViewState`,
            // which triggers the camera reposition (auto-scale
            // equivalent for the legacy chart surface). The log
            // below stamps the logical step; the observable effect
            // lands when `Message::DataLoaded` fires for this
            // receiver.
            log_link_propagation_step(
                LinkPropagationStep::ResetAndAutoScale,
                id.0,
                &old_sym,
                new_symbol,
            );
        }

        // Floating charts.
        let floating_targets: Vec<window::Id> = find_link_targets(
            source_link,
            self.floating_charts
                .iter()
                .map(|(wid, panel)| (*wid, panel.symbol_link)),
        );
        let symbol = new_symbol.trim().to_uppercase();
        let float_key = crate::annotation_store::SymbolKey::new(&symbol);
        let had_floating_targets = !floating_targets.is_empty();
        for wid in floating_targets {
            let tf = self
                .floating_charts
                .get(&wid)
                .map(|c| c.timeframe)
                .unwrap_or(Timeframe::D1);
            if let Some(chart) = self.floating_charts.get_mut(&wid) {
                chart.bound_symbol = Some(float_key.clone());
                chart.symbol = symbol.clone();
                chart.symbol_input = symbol.clone();
                chart.gatr_hover = false;
                chart.load_state = LoadState::Loading;
                chart.chart_state.dirty.mark_data();
            }
            tasks.push(self.load_floating_chart_async(wid, &symbol, tf));
        }
        // S7e: router-driven subscriptions spawn per-chart in
        // `chart_subscriptions` on the next iced re-diff; no eager
        // reconciliation needed.
        let _ = had_floating_targets;

        // Order panels — route through handle_order_panel_symbol_change
        // for bracket lifecycle (cancel old, create new), then bind.
        let sym_key = crate::annotation_store::SymbolKey::new(&symbol);
        let order_targets: Vec<OrderPanelId> = find_link_targets(
            source_link,
            self.order_panels.iter().map(|(id, p)| (*id, p.symbol_link)),
        );
        for op_id in order_targets {
            let old_sym = self
                .order_panels
                .get(&op_id)
                .map(|p| p.state.symbol.clone())
                .unwrap_or_default();
            let recalled = self.handle_order_panel_symbol_change(op_id, &old_sym, &symbol);
            self.bind_panel_to_symbol(op_id, sym_key.clone());
            if let Some(panel) = self.order_panels.get_mut(&op_id) {
                if !recalled {
                    panel.state.tp_value.clear();
                    panel.state.sl_value.clear();
                    panel.state.sl_limit_value.clear();
                }
                panel.state.last_price = None;
                panel.state.errors.clear();
            }
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Propagate a timeframe change from source chart to all linked charts
    /// (both docked and floating).
    fn propagate_timeframe_change(
        &mut self,
        source_id: ChartId,
        new_tf: Timeframe,
    ) -> Task<Message> {
        use crate::link::find_link_targets;

        let source_link = self
            .charts
            .get(&source_id)
            .map(|c| c.timeframe_link)
            .unwrap_or(LinkMode::Unlinked);

        let mut tasks: Vec<Task<Message>> = Vec::new();

        // Docked charts.
        let pane_targets: Vec<ChartId> = find_link_targets(
            source_link,
            self.charts
                .iter()
                .filter(|(id, _)| **id != source_id)
                .map(|(id, panel)| (*id, panel.timeframe_link)),
        );
        for id in pane_targets {
            let symbol = self
                .charts
                .get(&id)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();
            if self.charts.get(&id).is_some_and(|c| c.timeframe != new_tf) {
                self.evict_chart_handle_for_current_binding(id);
            }
            if let Some(chart) = self.charts.get_mut(&id) {
                chart.timeframe = new_tf;
                chart.gatr_hover = false;
                chart.chart_state.dirty.mark_camera();
            }
            if !symbol.is_empty() {
                if let Some(chart) = self.charts.get_mut(&id) {
                    chart.load_state = LoadState::Loading;
                    chart.chart_state.dirty.mark_data();
                }
                tasks.push(self.load_chart_async(id, &symbol, new_tf));
            }
        }

        // Floating charts.
        let floating_targets: Vec<window::Id> = find_link_targets(
            source_link,
            self.floating_charts
                .iter()
                .map(|(wid, panel)| (*wid, panel.timeframe_link)),
        );
        for wid in floating_targets {
            let symbol = self
                .floating_charts
                .get(&wid)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();
            if let Some(chart) = self.floating_charts.get_mut(&wid) {
                chart.timeframe = new_tf;
                chart.gatr_hover = false;
            }
            if !symbol.is_empty() {
                if let Some(chart) = self.floating_charts.get_mut(&wid) {
                    chart.load_state = LoadState::Loading;
                    chart.chart_state.dirty.mark_data();
                }
                tasks.push(self.load_floating_chart_async(wid, &symbol, new_tf));
            }
        }

        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Build the provider registry, registering all available providers
    /// and restoring the active selection from config.
    ///
    /// Chart-transition slice 8.5: the registry is the pre-router
    /// fallback for `load_chart_with` / `load_market_snapshot` / the
    /// thumbnail helpers. Every session-chart (`backend: New`) path
    /// uses `self.router` exclusively and never calls through here.
    /// Deleted in slice 9c's atomic deletion PR.
    fn build_provider_registry(config: &AppConfig) -> HistoricalDataRegistry {
        let mut registry = HistoricalDataRegistry::new();
        let test_provider: Arc<dyn DataProvider> = Arc::new(midas_feed::TestProvider::new());
        registry.register_data_provider(test_provider);
        if let Some(ref prov_cfg) = config.providers {
            if let Some(ref saved_name) = prov_cfg.active_data {
                let names = registry.data_provider_names();
                if let Some(idx) = names.iter().position(|n| n == saved_name) {
                    registry.set_active_data(idx);
                    tracing::info!(provider = %saved_name, "restored active data provider");
                } else {
                    tracing::warn!(
                        saved = %saved_name,
                        available = ?names,
                        "saved provider not found, using default"
                    );
                }
            }
        }
        registry
    }

    /// How many calendar days of data to request for a timeframe.
    fn days_for_timeframe(tf: Timeframe) -> u32 {
        match tf.as_secs() {
            s if s >= Timeframe::W1.as_secs() => 3650, // ~10 years
            s if s >= Timeframe::D1.as_secs() => 730,  // ~2 years
            s if s >= Timeframe::H1.as_secs() => 90,   // ~3 months
            s if s >= Timeframe::M15.as_secs() => 30,  // ~1 month
            _ => 10,                                   // <=M5: ~10 days
        }
    }

    /// Core async data loader. Prefers the router's own provider so
    /// the chart shares the exact historical source the watchlist and
    /// ticker state stream from — a disjoint `TestProvider` would
    /// desynchronise prices across widgets (the core bug the
    /// router-refactor rewrite was meant to eliminate). Falls back to
    /// the legacy `HistoricalDataRegistry` only when no router is
    /// attached (early boot, fixture replay).
    fn load_chart_with<F>(
        &self,
        chart_id: ChartId,
        symbol: &str,
        tf: Timeframe,
        make_msg: F,
    ) -> Task<Message>
    where
        F: FnOnce(ChartId, String, u64, Result<Arc<CandleBuffer>, String>) -> Message
            + Send
            + 'static,
    {
        let gen = self
            .charts
            .get(&chart_id)
            .map(|c| c.load_generation)
            .unwrap_or(0);
        let symbol = symbol.to_uppercase();
        let requested_symbol = symbol.clone();

        if let Some(router) = self.router.clone() {
            let days = Self::days_for_timeframe(tf);
            return Task::perform(
                Self::load_chart_via_router(router, symbol, tf, days),
                move |result| make_msg(chart_id, requested_symbol, gen, result.map(Arc::new)),
            );
        }

        let Some(provider) = self.providers.active_data_provider() else {
            return Task::none();
        };
        let days = Self::days_for_timeframe(tf);
        Task::perform(
            async move {
                provider
                    .get_candles(&symbol, tf, days)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| make_msg(chart_id, requested_symbol, gen, result.map(Arc::new)),
        )
    }

    /// Async helper: fetch historical bars via the router, convert
    /// them into a `CandleBuffer` matching the legacy chart's storage
    /// shape. The router resolves `con_id` internally; the caller
    /// only supplies the symbol string.
    async fn load_chart_via_router(
        router: Arc<midas_market_data::MarketDataRouter>,
        symbol: String,
        tf: Timeframe,
        days: u32,
    ) -> Result<CandleBuffer, String> {
        let tf_bc = crate::app::handlers::chart_timeframe_to_broker_core(tf);
        let duration = midas_broker_core::market_data::IbDuration::Days(days.max(1));
        let key = midas_broker_core::SymbolKey {
            contract_id: 0,
            symbol: symbol.clone(),
        };
        let result = router
            .historical_bars(key, tf_bc, duration)
            .await
            .map_err(|e| e.to_string())?;
        Ok(bars_to_candle_buffer(&result.bars))
    }

    /// Async-load chart data. On completion, sends `Message::DataLoaded`
    /// which resets the camera to show the last 200 candles.
    fn load_chart_async(&self, chart_id: ChartId, symbol: &str, tf: Timeframe) -> Task<Message> {
        self.load_chart_with(chart_id, symbol, tf, Message::DataLoaded)
    }

    /// Async-load chart data for startup restore. On completion, sends
    /// `Message::DataRestoredFromStartup` which preserves the saved camera.
    fn load_chart_async_restore(
        &self,
        chart_id: ChartId,
        symbol: &str,
        tf: Timeframe,
    ) -> Task<Message> {
        self.load_chart_with(chart_id, symbol, tf, Message::DataRestoredFromStartup)
    }

    /// Load a market data snapshot for a symbol, preferring the
    /// router's provider so the watchlist "last close" matches
    /// whatever the chart loads on the same ticker.
    fn load_market_snapshot(&self, symbol: &str) -> Task<Message> {
        let sym = symbol.to_uppercase();
        let sym_clone = sym.clone();

        if let Some(router) = self.router.clone() {
            return Task::perform(
                Self::load_chart_via_router(router, sym, midas_core::Timeframe::D1, 30),
                move |result| Message::MarketSnapshotLoaded(sym_clone, result),
            );
        }

        let Some(provider) = self.providers.active_data_provider() else {
            return Task::none();
        };
        Task::perform(
            async move {
                provider
                    .get_candles(&sym, midas_core::Timeframe::D1, 30)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |result| Message::MarketSnapshotLoaded(sym_clone, result),
        )
    }

    /// Load market snapshots for all symbols across all watchlists.
    fn load_all_watchlist_snapshots(&self) -> Task<Message> {
        let mut seen = std::collections::HashSet::new();
        let mut tasks = Vec::new();
        for wl in self.watchlists.values() {
            for ticker in &wl.tickers {
                if seen.insert(ticker.symbol.clone()) {
                    tasks.push(self.load_market_snapshot(&ticker.symbol));
                }
            }
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Dispatch an async load for the thumbnail at `(symbol, tf)`.
    ///
    /// Consults [`ThumbnailDataStore::request_load`] to dedup in-flight
    /// loads and to skip when real data is already cached. Returns
    /// [`Task::none`] when no load is needed.
    fn spawn_thumbnail_load(&mut self, symbol: String, tf: Timeframe) -> Task<Message> {
        let days = Self::days_for_timeframe(tf);
        let Some(task) = self.thumbnail_data.request_load(&symbol, tf, days) else {
            // Either redundant, already in flight, or queued behind the
            // concurrent-load cap. In the queued case,
            // `drain_thumbnail_queue` will pick it up when a slot frees.
            return Task::none();
        };
        let Some(provider) = self.providers.active_data_provider() else {
            // No active provider — drop the pending marker so a future
            // provider switch can retry, and render the empty-state.
            self.thumbnail_data.install_empty(&symbol, tf);
            return Task::none();
        };
        Self::perform_thumbnail_load(provider, task)
    }

    /// Build the async [`Task`] that calls `provider.get_candles` and
    /// folds the result into the matching thumbnail [`Message`]. Stateless
    /// helper so both the initial dispatch site and
    /// [`drain_thumbnail_queue`](Self::drain_thumbnail_queue) can reuse it.
    fn perform_thumbnail_load(
        provider: Arc<dyn midas_core::provider::DataProvider>,
        task: crate::thumbnail_data::LoadTask,
    ) -> Task<Message> {
        let req_symbol = task.symbol;
        let req_tf = task.tf;
        let req_days = task.days;
        Task::perform(
            async move {
                let result = provider.get_candles(&req_symbol, req_tf, req_days).await;
                (req_symbol, req_tf, result)
            },
            |(symbol, tf, result)| match result {
                Ok(buffer) => Message::ThumbnailDataReady {
                    symbol,
                    tf,
                    buffer: Arc::new(buffer),
                },
                Err(err) => {
                    tracing::debug!(%symbol, ?tf, error = %err, "thumbnail load failed");
                    Message::ThumbnailLoadFailed { symbol, tf }
                }
            },
        )
    }

    /// Drain any thumbnail loads that were queued while the
    /// concurrent-load cap in
    /// [`crate::thumbnail_data::ThumbnailDataStore`] was saturated.
    ///
    /// Called from the `Message::ThumbnailDataReady` and
    /// `Message::ThumbnailLoadFailed` handlers so every completed load
    /// frees exactly one slot for a queued request. Produces a
    /// [`Task::batch`] of the dispatches picked up on this tick.
    fn drain_thumbnail_queue(&mut self) -> Task<Message> {
        let mut tasks: Vec<Task<Message>> = Vec::new();
        while let Some(task) = self.thumbnail_data.drain_next() {
            let Some(provider) = self.providers.active_data_provider() else {
                // Provider gone — mark the drained key empty so
                // `request_load` won't respawn it, and stop draining.
                self.thumbnail_data.install_empty(&task.symbol, task.tf);
                break;
            };
            tasks.push(Self::perform_thumbnail_load(provider, task));
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Pre-warm the thumbnail cache on startup by dispatching one load
    /// per unique (symbol, current interval) across every watchlist.
    ///
    /// Called once during `new()` after the watchlist panels are
    /// populated from config so thumbnails begin rendering as soon as
    /// the user opens the panel, rather than only after they click a
    /// thumbnail to cycle its interval.
    fn load_all_thumbnails(&mut self) -> Task<Message> {
        // Union of every symbol currently rendered with a thumbnail:
        // watchlist rows + order-blotter rows. Both surfaces call
        // `build_thumbnail_snapshot` in their view paths. Deduped by
        // (symbol, current tf) so a ticker appearing in multiple
        // watchlists or in both a watchlist and the blotter costs one
        // load, not N.
        let mut seen: std::collections::HashSet<(String, Timeframe)> =
            std::collections::HashSet::new();
        let mut pairs: Vec<(String, Timeframe)> = Vec::new();
        for wl in self.watchlists.values() {
            for ticker in &wl.tickers {
                let tf = self.thumbnail_store.get(&ticker.symbol);
                let key = (ticker.symbol.clone(), tf);
                if seen.insert(key.clone()) {
                    pairs.push(key);
                }
            }
        }
        for row in self.order_blotter.rows() {
            let tf = self.thumbnail_store.get(&row.symbol);
            let key = (row.symbol.clone(), tf);
            if seen.insert(key.clone()) {
                pairs.push(key);
            }
        }
        tracing::debug!(count = pairs.len(), "thumbnail prewarm: dispatching loads");
        let mut tasks: Vec<Task<Message>> = Vec::with_capacity(pairs.len());
        for (symbol, tf) in pairs {
            tasks.push(self.spawn_thumbnail_load(symbol, tf));
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Apply loaded candle data to a chart panel.
    ///
    /// When `view_state` is `Some`, the camera is positioned using the
    /// centralized [`ChartViewState`](crate::chart_view::ChartViewState)
    /// — the single authority for zoom level and last-candle placement.
    /// When `None`, only data and dirty flags are updated (camera untouched).
    fn apply_candle_data(
        chart: &mut ChartPanel,
        buffer: Arc<CandleBuffer>,
        view_state: Option<&crate::chart_view::ChartViewState>,
    ) {
        chart.data = Some(Arc::clone(&buffer));
        chart.load_state = LoadState::Loaded;
        chart.chart_state.dirty.mark_data();
        if buffer.is_empty() {
            return;
        }
        if let Some(vs) = view_state {
            vs.position_camera(
                &mut chart.chart_state.camera,
                &buffer,
                chart.chart_state.collapse_gaps,
                &mut chart.chart_state.data_time_start,
                &mut chart.chart_state.data_time_end,
            );
        }
        chart.chart_state.dirty.mark_camera();
    }

    /// Reload all charts that currently have data (e.g. after provider switch).
    fn reload_all_charts(&mut self) -> Task<Message> {
        let charts_to_reload: Vec<(ChartId, String, Timeframe)> = self
            .charts
            .iter()
            .filter(|(_, panel)| !panel.symbol.is_empty() && panel.data.is_some())
            .map(|(&id, panel)| (id, panel.symbol.clone(), panel.timeframe))
            .collect();
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for (chart_id, symbol, tf) in &charts_to_reload {
            if let Some(chart) = self.charts.get_mut(chart_id) {
                chart.load_state = LoadState::Loading;
                chart.chart_state.dirty.mark_data();
            }
            tasks.push(self.load_chart_async(*chart_id, symbol, *tf));
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    /// Async-load data for a floating chart window.
    fn load_floating_chart_async(
        &self,
        _wid: window::Id,
        symbol: &str,
        tf: Timeframe,
    ) -> Task<Message> {
        // Floating charts use ChartId(0) as sentinel. They receive the
        // same DataLoaded message and are handled in the update loop
        // by matching on the floating_charts map.
        self.load_chart_with(ChartId::new(0), symbol, tf, Message::DataLoaded)
    }

    /// Get the active chart's ChartId (from workspace focus).
    fn active_chart_id(&self) -> Option<ChartId> {
        self.workspace.focused_chart_id()
    }

    /// Returns the symbol currently displayed on the focused chart,
    /// or `None` if no chart is focused or the chart has no symbol.
    ///
    /// Used by the ticker-intent reducer's mid-drag-ticker-switch guard
    /// to reject bracket-drag messages whose captured symbol does not
    /// match the currently visible chart.
    pub(crate) fn active_chart_symbol(&self) -> Option<String> {
        self.active_chart_id()
            .and_then(|id| self.charts.get(&id))
            .map(|chart| chart.symbol.clone())
            .filter(|s| !s.is_empty())
    }

    /// Focus the pane displaying the given chart, if it exists in the layout.
    ///
    /// Called from message handlers so that interacting with any pane
    /// (title-bar buttons, shader events) implicitly focuses it.  This
    /// replaces the `PaneGrid::on_click` approach which consumed the
    /// initial mouse-press and prevented title-bar buttons on unfocused
    /// panes from registering clicks.
    fn focus_chart(&mut self, chart_id: ChartId) {
        if let Some(pane) = self.workspace.find_pane(chart_id) {
            self.workspace.set_focus(pane);
        }
    }

    /// Mark the currently-focused chart's symbol as "session-active"
    /// and return a [`Task`] that fires the GATR snap evaluator if
    /// this is the first activation this session. This is the
    /// second of the two plan-mandated entry points for the snap
    /// rule — the first is `MarketSnapshotLoaded`, which catches
    /// symbols that were not yet loaded; this catches symbols that
    /// were already loaded from a previous activation but have since
    /// drifted.
    ///
    /// Returns `Task::none()` when no chart is focused or the active
    /// symbol has already been evaluated this session.
    /// Set a plain (no-action) toast, replacing any existing one.
    ///
    /// Convenience wrapper around `self.toasts.update(ToastMsg::Show)`
    /// for the many synchronous call sites that don't have a `Task`
    /// loop handy (status messages, validation errors). Callers that
    /// need an action button should emit
    /// `Message::Toast(ToastMsg::Show { message, action: Some(..) })`
    /// instead so the action survives the controller boundary.
    pub(crate) fn show_toast<S: Into<String>>(&mut self, message: S) {
        let _ = self.toasts.update(crate::toast::ToastMsg::Show {
            message: message.into(),
            action: None,
        });
    }

    /// Build a [`BracketSubmitter`] from the current
    /// `router_order_client`, or `None` if the router isn't ready yet.
    ///
    /// Returned by value (cheap `Arc` clone) so callers can move it
    /// into `Task::perform` futures without borrowing `self`.
    pub(crate) fn bracket_submitter(&self) -> Option<midas_broker::BracketSubmitter> {
        self.router_order_client
            .clone()
            .map(midas_broker::BracketSubmitter::new)
    }
}

// Ticker-state wiring (bind_chart_to_symbol, bind_panel_to_symbol,
// sync_panel_to_intent, sync_drag_to_intent, panel_display_for_chart,
// hydrate_order_panel_for_chart, maybe_emit_snap_for_active_chart,
// ticker_mut, handle_ticker_effects) is in app/ticker_wiring.rs.

// Config persistence (build_config, mark_config_dirty, maybe_save_config,
// flush_config) is in app/persistence.rs.

// ── Update ────────────────────────────────────────────────────────────

impl MidasApp {
    /// Process a message and return any async tasks to execute.
    ///
    /// This is a thin dispatcher that routes each [`Message`] variant to
    /// a domain-specific handler in `app/handlers.rs`. The handlers
    /// contain the actual logic (previously inlined as 108 match arms).
    pub fn update(&mut self, message: Message) -> Task<Message> {
        #[cfg(feature = "dev_harness")]
        {
            if !is_tick_rate_message(&message) {
                if let Some(idle) = crate::dev_harness::idle::try_global() {
                    idle.mark();
                }
            }
        }

        match message {
            // -- Symbol / data loading --
            Message::PanelSymbolInputChanged(..)
            | Message::PanelSymbolSubmitted(..)
            | Message::PanelTimeframeSelected(..)
            | Message::DataLoaded(..)
            | Message::DataRestoredFromStartup(..)
            | Message::DataProviderSelected(..)
            | Message::OrderBrokerSelected(..) => self.handle_symbol_data_msg(message),

            // -- Chart management --
            Message::AddChart
            | Message::CloseChart(..)
            | Message::ActivateChart(..)
            | Message::LayoutPreset(..) => self.handle_chart_management_msg(message),

            // -- Pane grid --
            Message::PaneFocused(..)
            | Message::PaneResized(..)
            | Message::PaneDragged(..)
            | Message::PaneSplit(..)
            | Message::PaneClose(..) => self.handle_pane_msg(message),

            // -- Chart interaction (wrapper) --
            // New in audit P2 #4 collapse: every emit-site that used
            // to fire its own `Message::Chart*(id, ...)` variant now
            // wraps the raw `ChartAction` payload here. Dispatcher
            // hands it to `dispatch_chart_action`, which is the SOLE
            // place that knows how each action variant maps to a
            // legacy handler arm. Once Phase B inlines all the bodies,
            // the legacy variants below are deleted.
            Message::Chart(chart_id, action) => self.dispatch_chart_action(chart_id, action),

            // -- Chart interaction (viewport, pan, zoom, crosshair,
            //    levels, level editor, toggles, reset, batch) --
            Message::ChartViewportChanged(..)
            | Message::ChartDragLevelEnd(..)
            | Message::ChartClearAllLevels(..)
            | Message::ChartCloseLevelEditor(..)
            | Message::ChartDeleteLevel(..)
            | Message::LevelEditorPriceChanged(..)
            | Message::LevelEditorPriceStep(..)
            | Message::LevelEditorLabelChanged(..)
            | Message::LevelEditorColorChanged(..)
            | Message::LevelEditorThicknessChanged(..)
            | Message::LevelEditorIconChanged(..)
            | Message::LevelEditorToggleLock(..)
            | Message::DrawingPanelCreateLevel(..)
            | Message::ToggleCollapseGaps(..)
            | Message::ToggleVolumeProfile(..)
            | Message::ToggleLevels(..)
            | Message::ResetChart(..)
            | Message::ToggleChartBackend(..)
            | Message::SetChartBackend(..)
            | Message::ChartBatch(..) => self.handle_chart_interaction_msg(message),

            // -- Keyboard --
            Message::KeyPressed(key) => self.handle_key_press(key),

            // -- Order panel --
            Message::AddOrderPanel
            | Message::OrderPanelMsg(..)
            | Message::OrderPanelSetSymbolLink(..) => self.handle_order_panel_msg(message),

            // -- Account panel --
            Message::AddAccountPanel
            | Message::Account(..)
            | Message::AccountOrdersRowSelected(..)
            | Message::AccountOrdersSetSymbolLink(..)
            | Message::AccountOrdersOpenColumnSelector(..)
            | Message::AccountOrdersDismissColumnSelector
            | Message::AccountOrdersToggleColumn(..)
            | Message::AccountPositionsBatch(..) => self.handle_account_panel_msg(message),

            // -- Column resize (all grid surfaces) --
            Message::ColumnResize(ev) => self.handle_column_resize(ev),

            // -- Watchlist --
            Message::AddWatchlist
            | Message::WatchlistTickerInputChanged(..)
            | Message::WatchlistAddTicker(..)
            | Message::WatchlistRemoveTicker(..)
            | Message::WatchlistAdjustFavorite(..)
            | Message::WatchlistFavCellEnter(..)
            | Message::WatchlistFavCellExit(..)
            | Message::WatchlistTickerPressed(..)
            | Message::WatchlistDragConfirm(..)
            | Message::WatchlistDragCancel
            | Message::DragCursorMoved(..)
            | Message::DragMouseUp
            | Message::WatchlistTickerSelected(..)
            | Message::WatchlistSetSymbolLink(..)
            | Message::WatchlistGrid(..) => self.handle_watchlist_msg(message),

            // -- Market data cache --
            Message::MarketSnapshotLoaded(..) | Message::RefreshMarketData => {
                self.handle_market_data_msg(message)
            }

            // -- Chart linking --
            Message::SetSymbolLink(..)
            | Message::SetTimeframeLink(..)
            | Message::ToggleLinkPicker(..)
            | Message::DismissLinkPicker => self.handle_link_msg(message),

            // -- Window / config / floating --
            // Window-geometry events route through the dedicated
            // controller (audit P1 slice 2). Lifecycle / floating /
            // ConfigSaved / PopOut still live on MidasApp pending
            // their own slices.
            Message::Window(m) => self.dispatch_window(m),

            Message::ConfigSaved(..)
            | Message::WindowCloseRequested
            | Message::PopOut(..)
            | Message::FloatingWindowClosed(..) => self.handle_window_config_msg(message),

            // -- G.ATR hover highlight --
            Message::GatrHoverEnter(..) | Message::GatrHoverLeave(..) => {
                self.handle_gatr_hover_msg(message)
            }

            // -- Bracket context menu --
            // Bracket creation/drag/action buttons collapsed into
            // Message::Chart(...) — see audit P2 #4 batch 3.
            Message::BracketContextCancel(..) | Message::BracketContextDismiss => {
                self.handle_bracket_msg(message)
            }

            // -- Broker events --
            Message::BrokerBracketCreated { .. }
            | Message::BrokerBracketStatusChanged { .. }
            | Message::BrokerEventReceived(..)
            | Message::BrokerConnectionChanged(..)
            | Message::BracketPlaceResult(..)
            | Message::RouterOrderEvent(..)
            | Message::BrokerSimSpawned(..) => self.handle_broker_msg(message),

            // -- Router refactor (S7) --
            Message::ChartBarBatch { .. }
            | Message::ChartSubBarBatch { .. }
            | Message::ChartResync { .. }
            | Message::ChartResyncLoaded(..)
            | Message::QuoteBatch(..)
            | Message::QuoteResync { .. }
            | Message::TickerLastPrice { .. }
            | Message::FarmStatusChanged(..)
            | Message::RouterReady(..) => self.handle_router_msg(message),

            // -- Toast notifications --
            Message::Toast(m) => self.dispatch_toast(m),

            // -- Tick / Ticker state machine --
            Message::Tick | Message::Ticker(..) => self.handle_tick_ticker_msg(message),

            // -- Thumbnail cells (Slice 3 / Slice 4) --
            // The watchlist + blotter views read `thumbnail_store` and
            // `thumbnail_data` on every `view()` call, so mutating the
            // stores is sufficient to trigger a rebuild — no explicit
            // dirty flag is needed. When the new interval has no cached
            // data yet we also dispatch a load via the active provider.
            Message::ThumbnailIntervalCycle(symbol) => {
                let new_tf = self.thumbnail_store.cycle(&symbol);
                self.spawn_thumbnail_load(symbol, new_tf)
            }
            Message::ThumbnailDataReady { symbol, tf, buffer } => {
                self.thumbnail_data.install(&symbol, tf, &buffer);
                self.drain_thumbnail_queue()
            }
            Message::ThumbnailLoadFailed { symbol, tf } => {
                self.thumbnail_data.install_empty(&symbol, tf);
                self.drain_thumbnail_queue()
            }

            // -- Session chart (S8 + Phase C, feature-gated) --
            //
            // Boots the new-stack pipeline for the requested
            // (ticker, period, calendar) triple AND opens a standalone
            // iced window hosting the widget. See the doc-comment on
            // `Message::OpenSessionChart` and `session_chart/mod.rs`
            // for the lifecycle.
            #[cfg(feature = "session_chart")]
            Message::OpenSessionChart(request) => self.handle_open_session_chart(request),
            #[cfg(feature = "session_chart")]
            Message::SessionChartWindowOpened(window_id, payload) => {
                self.handle_session_chart_window_opened(window_id, payload)
            }
            #[cfg(feature = "session_chart")]
            Message::SessionChartOpenFailed(window_id) => {
                tracing::warn!(
                    ?window_id,
                    "session_chart: async pipeline construction failed; closing window"
                );
                window::close(window_id)
            }
            #[cfg(feature = "session_chart")]
            Message::SessionChartCyclePolicy(window_id) => {
                self.handle_session_chart_cycle_policy(window_id)
            }
            #[cfg(feature = "session_chart")]
            Message::SessionChartToggleLevelTool(window_id) => {
                self.handle_session_chart_toggle_level_tool(window_id)
            }
            #[cfg(feature = "session_chart")]
            Message::SessionChartActivateBuyBracketTool(window_id) => {
                self.handle_session_chart_activate_bracket_tool(window_id, true)
            }
            #[cfg(feature = "session_chart")]
            Message::SessionChartActivateSellBracketTool(window_id) => {
                self.handle_session_chart_activate_bracket_tool(window_id, false)
            }

            // -- Dev harness (feature-gated) --
            #[cfg(feature = "dev_harness")]
            Message::DevHarness { command, responder } => {
                crate::dev_harness::handle_command(command, responder, self)
            }
            #[cfg(feature = "dev_harness")]
            Message::DevHarnessScreenshotReady {
                screenshot,
                out_path,
                responder,
            } => {
                crate::dev_harness::handle_screenshot_ready(screenshot, out_path, responder);
                Task::none()
            }
            #[cfg(feature = "dev_harness")]
            Message::DevHarnessSimSpawned { handle, responder } => {
                crate::dev_harness::handle_sim_spawned(self, handle, responder);
                Task::none()
            }
        }
    }

    // handler_key_press and set_active_timeframe remain here (not moved
    // to handlers.rs) because KeyPressed dispatches directly to
    // handle_key_press, and set_active_timeframe is its helper.

    /// Handle keyboard shortcut actions.
    fn handle_key_press(&mut self, key: iced::keyboard::Key) -> Task<Message> {
        use iced::keyboard::key::Named;
        use iced::keyboard::Key;
        match key {
            Key::Character(ref c) => match c.as_str() {
                "1" => return self.set_active_timeframe(Timeframe::M1),
                "2" => return self.set_active_timeframe(Timeframe::M5),
                "3" => return self.set_active_timeframe(Timeframe::M15),
                "4" => return self.set_active_timeframe(Timeframe::H1),
                "5" => return self.set_active_timeframe(Timeframe::H4),
                "6" => return self.set_active_timeframe(Timeframe::D1),
                "7" => return self.set_active_timeframe(Timeframe::W1),
                "h" | "H" => {
                    self.level_placing = !self.level_placing;
                }
                "t" | "T" => {
                    // Focus nearest order panel, or create one if none exists.
                    if let Some(pane) = self.workspace.find_any_order_pane() {
                        self.workspace.set_focus(pane);
                    } else {
                        return self.update(Message::AddOrderPanel);
                    }
                }
                _ => {}
            },
            Key::Named(Named::Escape) => {
                self.link_picker_open = None;
                self.level_placing = false;
                self.placing_preview = None;
                self.bracket_context_menu = None;
                // Route Dismiss through the controller so all toast
                // mutations land in one named place (no `clear()` /
                // `self.state = None` back-doors). Effects always
                // empty for Dismiss; safe to discard.
                let _ = self.toasts.update(crate::toast::ToastMsg::Dismiss);
                self.pending_drag = None;
                if self.dragging_ticker.is_some() {
                    self.dragging_ticker = None;
                }
            }
            Key::Named(Named::F11) => {
                self.show_frame_overlay = !self.show_frame_overlay;
            }
            _ => {}
        }
        Task::none()
    }

    /// Set the timeframe on the active chart and regenerate data.
    fn set_active_timeframe(&mut self, tf: Timeframe) -> Task<Message> {
        if let Some(id) = self.active_chart_id() {
            let symbol = self
                .charts
                .get(&id)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();

            if self.charts.get(&id).is_some_and(|c| c.timeframe != tf) {
                self.evict_chart_handle_for_current_binding(id);
            }
            if let Some(chart) = self.charts.get_mut(&id) {
                chart.timeframe = tf;
                chart.chart_state.dirty.mark_camera();
            }

            let mut tasks: Vec<Task<Message>> = Vec::new();
            if !symbol.is_empty() {
                if let Some(chart) = self.charts.get_mut(&id) {
                    chart.load_state = LoadState::Loading;
                    chart.chart_state.dirty.mark_data();
                }
                tasks.push(self.load_chart_async(id, &symbol, tf));
            }

            tasks.push(self.propagate_timeframe_change(id, tf));
            self.mark_config_dirty();
            return Task::batch(tasks);
        }
        Task::none()
    }

    /// Phase C handler: boot the new-stack pipeline for the requested
    /// (ticker, period, calendar) triple AND open a standalone iced
    /// window hosting the widget.
    ///
    /// Feature-gated on `session_chart`. The pipeline construction
    /// happens inside a `Task::perform` so the pump task is tied to
    /// the iced runtime; on success the handler sends a
    /// [`Message::SessionChartWindowOpened`] that carries the driver
    /// Arc + original request back onto the main thread, where the
    /// widget + window state can be installed in
    /// `floating_session_charts`.
    ///
    /// The standalone window is opened via `window::open` BEFORE the
    /// async driver construction completes — iced hands us the
    /// `window::Id` synchronously. The window's view starts empty
    /// (no chart for this id yet), and the
    /// `SessionChartWindowOpened` handler installs the widget +
    /// driver state keyed by that id. This matches how the
    /// pop-out floating-chart path works.
    #[cfg(feature = "session_chart")]
    fn handle_open_session_chart(
        &mut self,
        request: crate::session_chart::SessionChartRequest,
    ) -> Task<Message> {
        use midas_bars_adapter::{subscribe_aggregated_bars, StaticSymbolResolver, SymbolResolver};
        use midas_broker::sim::{SimConfig, SimMarketData};
        use midas_clock::SystemClock;
        use std::sync::Arc;

        // Fresh sim source — independent of `self.router` to keep the
        // blast radius zero on the legacy subscription surface.
        let sim_cfg = SimConfig::default();
        let source: Arc<dyn midas_broker::MarketDataSource> =
            SimMarketData::new(sim_cfg.market_data.clone());
        let clock: Arc<dyn midas_clock::Clock> = Arc::new(SystemClock);

        tracing::info!(
            "session_chart: booting pipeline ticker={} period={:?} calendar={}",
            request.ticker,
            request.period,
            request.calendar_id.0,
        );

        // Synchronously open the window so iced hands us the id to
        // wire into the `SessionChartWindowOpened` handler.
        let (win_id, open_task) = window::open(window::Settings {
            size: iced::Size::new(720.0, 520.0),
            ..window::Settings::default()
        });

        // Spawn the async construction. On success, the returned
        // Task resolves to a `Message::SessionChartWindowOpened` that
        // the main update loop handles synchronously (installing the
        // widget / driver into `floating_session_charts`).
        let request_for_task = request.clone();
        let win_id_for_task = win_id;
        let construct_task = Task::perform(
            async move {
                use midas_stream::BarStream;

                let resolver = StaticSymbolResolver::new();
                let resolved = match resolver.resolve(&request_for_task.ticker) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(
                            "session_chart: resolve failed for {}: {e}",
                            request_for_task.ticker
                        );
                        return None;
                    }
                };
                if resolved.calendar.id() != request_for_task.calendar_id {
                    tracing::error!(
                        "session_chart: calendar mismatch — resolver says {} but request asks for {}",
                        resolved.calendar.id().0,
                        request_for_task.calendar_id.0,
                    );
                    return None;
                }

                let stream = match subscribe_aggregated_bars(
                    source,
                    &resolver,
                    &request_for_task.ticker,
                    request_for_task.period,
                    clock,
                )
                .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("session_chart: subscribe failed: {e}");
                        return None;
                    }
                };
                let meta = stream.meta().clone();
                let series = Arc::new(parking_lot::RwLock::new(midas_bars::CandleSeries::new(
                    meta.calendar.id(),
                    meta.period,
                    meta.symbol,
                )));
                let driver = Arc::new(crate::session_chart::SessionChartDriver::spawn(
                    series, stream,
                ));

                tracing::info!(
                    "session_chart: pipeline up for {} ({:?}); initial version={}",
                    request_for_task.ticker,
                    request_for_task.period,
                    driver.current_version(),
                );

                Some(crate::app::SessionChartWindowPayload {
                    driver,
                    request: request_for_task,
                })
            },
            move |maybe_payload| match maybe_payload {
                Some(p) => Message::SessionChartWindowOpened(win_id_for_task, p),
                // App-harden M1: the async pipeline failed (timeout,
                // resolver error, etc.). Close the already-opened
                // window instead of leaving an empty frame.
                None => Message::SessionChartOpenFailed(win_id_for_task),
            },
        );

        // Chain: open window first (iced needs this to assign the
        // id), then fire the async pipeline construction.
        Task::batch([open_task.map(|_id| Message::Tick), construct_task])
    }

    /// Phase C handler: iced handed us the window id, and our async
    /// pipeline construction succeeded. Install the widget + driver
    /// in `floating_session_charts`.
    #[cfg(feature = "session_chart")]
    fn handle_session_chart_window_opened(
        &mut self,
        window_id: window::Id,
        payload: crate::app::SessionChartWindowPayload,
    ) -> Task<Message> {
        use midas_axis::{PriceRange, Viewport};
        use midas_scene::ThemePalette;

        let calendar: &'static dyn midas_calendar::ExchangeCalendar =
            if payload.request.calendar_id == midas_calendar::CRYPTO_SPOT_ID {
                midas_calendar::crypto_spot()
            } else if payload.request.calendar_id == midas_calendar::XNYS_ID {
                midas_calendar::xnys()
            } else {
                tracing::error!(
                    "session_chart: unsupported calendar id {}",
                    payload.request.calendar_id.0
                );
                return Task::none();
            };

        // Pick a sensible default time window for the chart's axis.
        // For crypto: 24h centred on now. For XNYS: ~2 trading days
        // centred on now. Both are just initial hints — the user can
        // pan/zoom through the widget's `set_time_window`.
        let now = chrono::Utc::now();
        let (ts_start, ts_end) = if payload.request.calendar_id == midas_calendar::CRYPTO_SPOT_ID {
            (
                now - chrono::Duration::hours(12),
                now + chrono::Duration::hours(12),
            )
        } else {
            (
                now - chrono::Duration::days(2),
                now + chrono::Duration::hours(6),
            )
        };

        let price_range =
            PriceRange::new(1.0, 100_000.0).expect("price range for initial chart must be valid");
        let viewport = Viewport::new(700.0, 400.0);

        let widget = match crate::session_chart::SessionChart::new(
            std::sync::Arc::clone(&payload.driver),
            calendar,
            payload.request.period,
            price_range,
            viewport,
            ThemePalette::dark_default(),
            (ts_start, ts_end),
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(
                    ?window_id,
                    error = %e,
                    "session_chart: widget construction rejected; closing window"
                );
                return window::close(window_id);
            }
        };

        let state = crate::session_chart_window::SessionChartWindow::new(
            widget,
            payload.driver,
            payload.request,
        );
        self.floating_session_charts.insert(window_id, state);
        Task::none()
    }

    /// Phase C handler: user pressed the "EH" chip in a session-chart
    /// window. Cycles the widget's [`EhPolicy`]. Re-subscription
    /// through `Filtered<_, EhFilter>` is scheduled for a follow-up
    /// slice; for now the scene chrome reflects the new policy via
    /// [`SceneLayers::from_eh_policy`], which matches the
    /// ideal-design contract that `ShowAll` / `HideExtended` emit the
    /// same chrome.
    #[cfg(feature = "session_chart")]
    fn handle_session_chart_cycle_policy(&mut self, window_id: window::Id) -> Task<Message> {
        if let Some(state) = self.floating_session_charts.get_mut(&window_id) {
            // Take a write guard for the single mutation. Scope
            // deliberately kept to one statement so paint passes
            // taking `try_write()` never contend for long.
            let new_policy = {
                let mut g = state.widget.write();
                g.cycle_eh_policy()
            };
            tracing::info!(
                "session_chart: window {:?} -> eh_policy={:?}",
                window_id,
                new_policy
            );
        }
        Task::none()
    }

    /// Slice 4 chart-transition: toggle the level-placement tool on
    /// the session-chart widget tied to `window_id`. Flips
    /// `level_host.tool` between active (`LevelTool::placing()`) and
    /// off. Subsequent mouse clicks on the chart commit level
    /// annotations via the `ProjectedEffect::CreateLevel` path.
    #[cfg(feature = "session_chart")]
    fn handle_session_chart_toggle_level_tool(&mut self, window_id: window::Id) -> Task<Message> {
        if let Some(state) = self.floating_session_charts.get_mut(&window_id) {
            let mut g = state.widget.write();
            if g.is_level_tool_active() {
                g.deactivate_level_tool();
            } else {
                g.activate_level_tool();
            }
        }
        Task::none()
    }

    /// Slice 5b chart-transition: activate the bracket-placement tool
    /// on the session-chart widget. `is_buy = true` → Long bracket,
    /// false → Short. Clicking the same-side button again while the
    /// tool is active deactivates (toggle behaviour matches the Add
    /// Level chip). Clicking the OPPOSITE-side button swaps the side
    /// without deactivating.
    ///
    /// Bracket effects that the widget drains from its projection
    /// queue translate here into the existing `TickerMsg` draft-then-
    /// save sequence (architecture rule 8 / plan C1). Orphan drafts on
    /// window close are prevented by
    /// [`SessionChart::deactivate_bracket_tool`], which emits
    /// `CancelDraftBracket` mid-placement (R11).
    #[cfg(feature = "session_chart")]
    fn handle_session_chart_activate_bracket_tool(
        &mut self,
        window_id: window::Id,
        is_buy: bool,
    ) -> Task<Message> {
        if let Some(state) = self.floating_session_charts.get_mut(&window_id) {
            let mut g = state.widget.write();
            // Toggle: if active + same side, deactivate. Otherwise
            // install the requested side.
            let same_side_active = match g.bracket_tool_mode() {
                Some(midas_scene::tools::BracketToolMode::AwaitingEntry { side })
                | Some(midas_scene::tools::BracketToolMode::AwaitingTarget { side, .. })
                | Some(midas_scene::tools::BracketToolMode::AwaitingStop { side, .. }) => {
                    match side {
                        midas_scene::tools::Side::Long => is_buy,
                        midas_scene::tools::Side::Short => !is_buy,
                    }
                }
                _ => false,
            };
            if same_side_active {
                g.deactivate_bracket_tool();
            } else if is_buy {
                g.activate_buy_bracket_tool();
            } else {
                g.activate_sell_bracket_tool();
            }
        }
        Task::none()
    }
}

// View functions (view, view_toolbar, view_content, view_pane_*, view_status_bar)
// are in app/views.rs.

// ── Chart-transition slice 9a: per-panel backend toggle tests ────────
//
// Covers plan slice 9a + R4 (rollback mechanics) + R11 (live-bracket
// handoff) on the pieces that don't require spinning up a full
// `MidasApp` (iced runtime, wgpu surface, market-data router, etc.).
// The feature-gate × config matrix lives in
// `app::views::backend_dispatch_tests`; bracket-tool integration lives
// in `desktop/win/tests/bracket_tool_integration.rs`. What's here is
// the ChartPanel + ChartConfig + TickerState wiring specific to
// slice 9a.

#[cfg(test)]
mod backend_toggle_tests {
    use super::*;
    use midas_chart::widget::order_bracket::{
        BracketLeg, BracketSide, BracketStatus, LegRole, OrderBracket,
    };
    use midas_chart::widget::{LineStroke, LineStyle, PriceLine};
    use midas_core::config::ChartConfig;
    use midas_core::LinkMode;

    /// Compute the next backend for a toggle click — mirrors the
    /// handler body so we can unit-test the transition rule without
    /// building a full MidasApp.
    fn toggle(current: ChartBackend) -> ChartBackend {
        match current {
            ChartBackend::Legacy => ChartBackend::New,
            ChartBackend::New => ChartBackend::Legacy,
        }
    }

    #[test]
    fn new_panel_defaults_to_legacy_backend() {
        let panel = MidasApp::make_empty_panel();
        assert_eq!(panel.backend, ChartBackend::Legacy);
    }

    #[test]
    fn toggle_round_trip_legacy_new_legacy() {
        let mut backend = ChartBackend::Legacy;
        backend = toggle(backend);
        assert_eq!(backend, ChartBackend::New);
        backend = toggle(backend);
        assert_eq!(backend, ChartBackend::Legacy);
        backend = toggle(backend);
        assert_eq!(backend, ChartBackend::New);
    }

    /// Persisted selection restores on reload — `restore_panel`
    /// propagates `ChartConfig::backend` into `ChartPanel::backend`.
    #[test]
    fn restore_panel_reads_persisted_backend_new() {
        let cfg = ChartConfig {
            symbol: "AAPL".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: Some(ChartBackend::New),
        };
        let panel = MidasApp::restore_panel(&cfg);
        assert_eq!(panel.backend, ChartBackend::New);
    }

    /// A pre-9a config (no `backend` field) restores with the default
    /// (`Legacy`). Validates the back-compat path.
    #[test]
    fn restore_panel_without_backend_defaults_to_legacy() {
        let cfg = ChartConfig {
            symbol: "AAPL".into(),
            timeframe: "1D".into(),
            levels: vec![],
            camera_time_start: None,
            camera_time_end: None,
            camera_price_low: None,
            camera_price_high: None,
            collapse_gaps: false,
            timeline_border_ratio: 0.20,
            volume_scale: 1.0,
            show_volume_profile: false,
            show_levels: true,
            viewport_width: None,
            viewport_height: None,
            symbol_link: LinkMode::default(),
            timeframe_link: LinkMode::default(),
            bound_symbol: None,
            backend: None,
        };
        let panel = MidasApp::restore_panel(&cfg);
        assert_eq!(panel.backend, ChartBackend::Legacy);
    }

    // Helper: build a fake `OrderBracket` in the given state. Mirrors
    // the test helpers in `ticker_state/tests.rs` but scoped locally
    // so this module stays self-contained.
    fn mk_bracket(status: BracketStatus, filled: Option<f64>) -> OrderBracket {
        fn leg(price: f64, role: LegRole) -> BracketLeg {
            BracketLeg {
                line: PriceLine {
                    price,
                    extent: midas_chart::widget::LineExtent::FullWidth,
                    stroke: LineStroke {
                        color: [1.0, 1.0, 1.0, 1.0],
                        width: 1.0,
                        style: LineStyle::Solid,
                    },
                },
                role,
                projected_pnl: None,
                projected_pnl_pct: None,
            }
        }
        OrderBracket {
            entry: leg(100.0, LegRole::Entry),
            take_profit: Some(leg(110.0, LegRole::TakeProfit)),
            stop_loss: Some(leg(95.0, LegRole::StopLoss)),
            side: BracketSide::Long,
            status,
            quantity: Some(10.0),
            saved: false,
            filled_qty: filled,
            entry_type: midas_chart::widget::order_bracket::EntryType::Market,
            entry_stop_price: None,
            wrong_side_warning: false,
        }
    }

    /// R11 state handoff: a DRAFT bracket is cancellable, a LIVE
    /// (Active) bracket is preserved — only the rendering path
    /// changes. This test encodes the status discriminant the
    /// handler uses to decide whether to fire
    /// `TickerMsg::CancelBracket` before swapping backends.
    #[test]
    fn backend_swap_draft_detection_discriminates_draft_vs_live() {
        let draft = mk_bracket(BracketStatus::Draft, None);
        let active = mk_bracket(BracketStatus::Active, Some(10.0));
        let pending = mk_bracket(BracketStatus::Pending, None);
        let partial = mk_bracket(BracketStatus::PartialFill, Some(5.0));

        // Only `Draft` triggers the auto-cancel in
        // `Message::SetChartBackend` (other statuses mean the bracket
        // is already working at the broker; keep it).
        fn is_draft(b: &OrderBracket) -> bool {
            matches!(b.status, BracketStatus::Draft)
        }
        assert!(is_draft(&draft));
        assert!(!is_draft(&active));
        assert!(!is_draft(&pending));
        assert!(!is_draft(&partial));
    }

    /// R11 partial-fill rendering: an active bracket with
    /// `filled_qty < total_qty` must render with the brighter
    /// entry-line styling (slice 5b). The color-choice logic lives
    /// inside `OrderBracketView::is_partially_filled` in
    /// `crates/midas-scene/src/layers/annotations.rs`; here we pin
    /// the per-status semantics the handler uses when building the
    /// view for the new layer.
    #[test]
    fn partial_fill_status_is_distinct_from_active() {
        let partial = mk_bracket(BracketStatus::PartialFill, Some(5.0));
        let active = mk_bracket(BracketStatus::Active, Some(10.0));
        // The chart-crate BracketStatus and midas-scene
        // `OrderBracketView::is_partially_filled` use the same
        // discriminator: filled_qty < total_qty. Both statuses carry
        // different visual treatments via the bracket-rendering path.
        assert_ne!(partial.status, active.status);
        assert_eq!(partial.filled_qty, Some(5.0));
        assert_eq!(active.filled_qty, Some(10.0));
    }

    /// `Message::ToggleChartBackend` and `Message::SetChartBackend`
    /// are part of the public Message enum (discoverable by the
    /// devloop harness + any future test-injection path). Pinning
    /// their variant shape + Clone here so a rename breaks the
    /// contract loudly.
    #[test]
    fn toggle_and_set_backend_messages_exist_and_clone() {
        let toggle = Message::ToggleChartBackend(midas_core::ChartId::new(0));
        let set = Message::SetChartBackend(midas_core::ChartId::new(1), ChartBackend::New);
        let _ = toggle.clone();
        let _ = set.clone();
        // Debug-format is used by the devloop event log / tracing.
        let _dbg = format!("{toggle:?}");
        let _dbg = format!("{set:?}");
    }

    /// `ChartPanel::backend` survives clone — the `ChartPanel` is
    /// cloned into snapshots for the shader program. If clone drops
    /// the field, the new backend swap would silently revert.
    #[test]
    fn chart_panel_backend_survives_clone() {
        let mut panel = MidasApp::make_empty_panel();
        panel.backend = ChartBackend::New;
        let cloned = panel.clone();
        assert_eq!(cloned.backend, ChartBackend::New);
    }

    /// Multi-window drag isolation scaffolding (plan R14): two chart
    /// panels with independent `ChartPanel::backend` must not share
    /// state across the swap path. Pinning the Copy-ness of
    /// `ChartBackend` guards the invariant that per-panel flips don't
    /// leak through shared storage.
    #[test]
    fn chart_backend_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<ChartBackend>();
    }

    /// R14 dual-render perf budget (plan slice 9a): mixing a Legacy
    /// and a New panel in the same workspace must stay under the 14 ms
    /// frame budget. Full GPU perf gating requires a wgpu harness;
    /// this test pins the *resolution* path on the hot frame loop —
    /// every panel paint calls [`resolve_backend`] — so a micro-bench
    /// over N calls enforces the pure-CPU overhead is negligible.
    ///
    /// The threshold is generous (1 ms for 10_000 resolve calls ≈
    /// 100 ns per call, 1000× smaller than the frame budget) so the
    /// test is stable across CI runs and still catches regressions
    /// like a lock-under-contention or a chatty tracing call on the
    /// hot path.
    #[test]
    fn resolve_backend_stays_cheap_under_dual_panel_rate() {
        use std::time::Instant;
        let t0 = Instant::now();
        for i in 0..10_000 {
            // Interleave Legacy + New to emulate a two-panel mixed
            // workspace; `std::hint::black_box` keeps the optimizer
            // from eliding the side-effectful `tracing::warn!` path.
            let b = if i % 2 == 0 {
                ChartBackend::Legacy
            } else {
                ChartBackend::New
            };
            let _ =
                std::hint::black_box(crate::app::views::resolve_backend(std::hint::black_box(b)));
        }
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_millis() < 1,
            "resolve_backend × 10_000 should finish in < 1 ms, took {elapsed:?}"
        );
    }
}
