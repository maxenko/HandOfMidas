//! Application state, Message enum, and update logic.
//!
//! Sub-modules:
//! - `handlers`: domain-grouped message handlers (symbol/data, chart,
//!   pane, order panel, watchlist, broker, bracket, toast, etc.)
//! - `views`: widget tree construction (toolbar, pane grid, status bar)
//! - `persistence`: config build, save, and debounce

#[cfg(feature = "dev_harness")]
mod fixture;
mod handlers;
mod persistence;
mod ticker_wiring;
mod views;

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
    AccountPanelId, CandleBuffer, ChartId, DataProvider, LinkMode, OrderPanelId, Timeframe,
    WatchlistId,
};

use crate::registry::ProviderRegistry;

use crate::annotation_store::AnnotationStore;
use crate::layout::{LayoutPresetKind, PanelContent, WorkspaceLayout};
use crate::level_store::LevelStore;
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
    /// The window ID of the main application window (set after daemon opens it).
    pub main_window: Option<window::Id>,
    /// Floating chart windows popped out from the main pane grid.
    /// Keyed by the OS window ID returned from `window::open()`.
    pub floating_charts: HashMap<window::Id, ChartPanel>,
    /// Last known main window position (logical pixels, updated on move).
    pub window_position: Option<(i32, i32)>,
    /// Last known main window size (logical pixels, updated on resize).
    pub window_size: (u32, u32),
    /// Size of the monitor the main window is on (for config persistence).
    pub monitor_size: Option<(u32, u32)>,
    /// Centralized per-ticker level store, shared across all charts.
    pub level_store: LevelStore,
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
    /// Registry of all available data providers and order brokers.
    pub providers: ProviderRegistry,
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
    /// Active column resize: (watchlist_id, column_index, start_x, original_width).
    pub resizing_column: Option<(WatchlistId, usize, f32, f32)>,
    /// Active Account-pane Orders-tab column resize:
    /// (account_panel_id, column_index, start_x, original_width).
    pub resizing_account_column: Option<(AccountPanelId, usize, f32, f32)>,
    /// Active Account-pane History-tab column resize.
    /// Same shape as `resizing_account_column` but targets the History
    /// tab's `GridState` instead of Orders'.
    pub resizing_account_history_column: Option<(AccountPanelId, usize, f32, f32)>,
    /// Active Account-pane Recents-tab column resize. Two-column grid
    /// (ticker + last-seen); same shape otherwise.
    pub resizing_account_recents_column: Option<(AccountPanelId, usize, f32, f32)>,
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
    /// - The coalesced `positions_subscription` stream (steady-state),
    ///   which delivers `Message::AccountPositionsBatch`.
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
    /// Floating toast notification state. `None` when no toast is
    /// currently visible. Replaces the previous
    /// Toast notification controller. State + auto-dismiss + view all
    /// live behind [`crate::toast::ToastController`]; `MidasApp` only
    /// routes [`Message::Toast`] into it and interprets the resulting
    /// effects.
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
    /// Bridge to the midas-broker engine. None if engine failed to start.
    pub broker_bridge: Option<Arc<crate::broker_bridge::BrokerBridge>>,
    /// Current broker connection state display string.
    pub broker_connection_display: String,
    /// Symbols for which the GATR snap rule has already been evaluated
    /// once in the current session. Populated by the
    /// `MaybeSnapToGatr` path (startup + chart-activation hook) to
    /// avoid a second snap firing when the user tab-cycles back to a
    /// ticker they already looked at. Deliberately not persisted — a
    /// crash-and-relaunch resets it.
    pub snapped_this_session: std::collections::HashSet<crate::annotation_store::SymbolKey>,
    /// Discoverability-toast guard: symbols for which the one-shot
    /// "bracket location recorded" toast has already been emitted this
    /// session. Prevents the toast from re-firing on every subsequent
    /// panel edit for the same anchor seed.
    #[allow(dead_code)] // used by future snap toast deduplication
    pub anchor_seed_toasts_shown: std::collections::HashSet<crate::annotation_store::SymbolKey>,
    /// Pre-snap undo slot, populated when the GATR snap rule fires
    /// and drained when the user clicks the `Undo` action button on
    /// the snap toast. 30-second session-bounded TTL enforced at
    /// drain time — stale entries are silently discarded. Keyed by
    /// symbol so a second snap on the same ticker replaces the prior
    /// undo slot instead of stacking.
    #[allow(dead_code)] // used by future undo-snap UI path
    pub gatr_undo_slots: std::collections::HashMap<
        crate::annotation_store::SymbolKey,
        crate::ticker_state::PreSnapState,
    >,
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
    /// Viewport dimensions changed (old_w, old_h, new_w, new_h).
    /// Adjusts camera data range to preserve candle scale.
    ChartViewportChanged(ChartId, u32, u32, u32, u32),
    /// Pan the chart camera by a data-space delta.
    ChartPan(ChartId, f64, f64),
    /// Zoom the chart time axis (horizontal). Carries pivot_time (f64) and factor.
    ChartZoom(ChartId, f64, f64),
    /// Zoom the chart price axis (vertical). Carries pivot_price (f64) and factor.
    ChartZoomY(ChartId, f64, f64),
    /// Set or clear the crosshair position.
    ChartCrosshair(ChartId, Option<(f32, f32)>),
    /// Create a new horizontal price level.
    ChartCreateLevel(ChartId, f64),
    /// Drag a level to a new price.
    ChartDragLevel(ChartId, u64, f64),
    /// Emitted once when a chart widget exits its
    /// `InteractionMode::DraggingAnnotation` — clears the app-level
    /// `dragging_annotation` so sibling charts stop drawing the level
    /// in their drag-pass z-layer. Paired with `ChartDragLevel`, which
    /// sets the flag on every mouse-move during the drag.
    ChartDragLevelEnd(ChartId),
    /// Select a level.
    ChartSelectLevel(ChartId, u64),
    /// Deselect any selected level.
    ChartDeselectLevel(ChartId),
    /// Delete the currently selected level.
    ChartDeleteSelectedLevel(ChartId),
    /// Clear all levels from a chart.
    ChartClearAllLevels(ChartId),
    /// Cancel level placement mode (from widget Escape/right-click).
    ChartCancelPlacing(ChartId),
    /// Report cursor position during level placement (for ghost preview).
    PlacingCursorMoved(ChartId, f64),
    /// Set the timeline border position for a chart.
    ChartSetTimelineBorderRatio(ChartId, f64),
    /// Set the volume bar height multiplier for a chart.
    ChartSetVolumeScale(ChartId, f64),

    // -- Level editing --
    /// Right-click on a level — open the level editor popup.
    ChartRightClickLevel(ChartId, u64, f32, f32),
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
    /// The main window was opened by the daemon; store its ID.
    MainWindowOpened(window::Id),
    /// A floating window was closed by the user.
    FloatingWindowClosed(window::Id),

    // -- Window geometry --
    /// Main window was moved (logical position).
    WindowMoved(i32, i32),
    /// Main window was resized (logical size).
    WindowResized(u32, u32),
    /// Monitor size query result.
    MonitorSizeResult(Option<iced::Size>),

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
    /// User started dragging a column divider in a watchlist header.
    WatchlistColumnResizeStart(WatchlistId, usize, f32),
    /// User is dragging a column divider — cursor at this x position.
    WatchlistColumnResizing(f32),
    /// User released the column divider drag.
    WatchlistColumnResizeEnd,
    /// Grid chrome event from a watchlist.
    WatchlistGrid(WatchlistId, midas_grid::GridMessage),

    // -- Chart linking --
    /// Set the symbol link mode for a docked chart.
    SetSymbolLink(ChartId, LinkMode),
    /// Set the timeframe link mode for a docked chart.
    SetTimeframeLink(ChartId, LinkMode),
    /// Set the symbol link mode for a floating chart.
    FloatingSetSymbolLink(window::Id, LinkMode),
    /// Set the timeframe link mode for a floating chart.
    FloatingSetTimeframeLink(window::Id, LinkMode),
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
    /// Begin a column-resize drag on the Orders tab. `col_idx` indexes
    /// into `OrderBlotterColumn::ALL` (schema reused for wire-compat).
    AccountOrdersColumnResizeStart(AccountPanelId, usize),
    /// Cursor-move while a column resize is active (x in logical pixels).
    AccountOrdersColumnResizing(f32),
    /// Drag released — commit width + mark config dirty.
    AccountOrdersColumnResizeEnd,
    /// Open the column-selector popup for an Account pane's Orders tab.
    AccountOrdersOpenColumnSelector(AccountPanelId),
    /// Dismiss the column-selector popup.
    AccountOrdersDismissColumnSelector,
    /// Toggle visibility of a single column in the given Account pane.
    AccountOrdersToggleColumn(AccountPanelId, midas_grid::ColumnId),
    /// Begin a column-resize drag on the Trade History tab. `col_idx`
    /// indexes into `HistoryColumn::ALL`. Widths are runtime-only in v1
    /// (no config persistence) per plan Decision 4.
    AccountHistoryColumnResizeStart(AccountPanelId, usize),
    /// Cursor-move while a History-tab column resize is active.
    AccountHistoryColumnResizing(f32),
    /// Drag released — commit width.
    AccountHistoryColumnResizeEnd,
    /// Begin a column-resize drag on the Recents tab. `col_idx` indexes
    /// into the Recents column list (0 = ticker, 1 = last-seen).
    AccountRecentsColumnResizeStart(AccountPanelId, usize),
    /// Cursor-move while a Recents-tab column resize is active.
    AccountRecentsColumnResizing(f32),
    /// Drag released — commit width.
    AccountRecentsColumnResizeEnd,
    /// Coalesced batch of position updates from the broker subscription.
    /// App-wide (not per-panel) because the `PositionStore` is shared
    /// across every open Account pane.
    AccountPositionsBatch(Vec<crate::account_panel::PositionRaw>),

    // -- G.ATR hover highlight --
    /// Mouse entered the G.ATR badge on a chart — activate candle dimming.
    GatrHoverEnter(ChartId),
    /// Mouse left the G.ATR badge — deactivate candle dimming.
    GatrHoverLeave(ChartId),

    // -- Bracket creation from drawing tool --
    /// Bracket tool completed a 3-click bracket on a chart.
    ChartCreateBracket(
        ChartId,
        f64,
        f64,
        f64,
        midas_chart::widget::order_bracket::BracketSide,
    ),

    // -- Bracket drag --
    /// A bracket leg was dragged on a chart.
    ChartDragBracketLeg(
        ChartId,
        u64,
        midas_chart::widget::order_bracket::LegRole,
        f64,
    ),

    // -- Bracket action buttons (from chart hit zones) --
    /// Submit bracket from chart button.
    ChartBracketSubmit(ChartId, AnnotationId),
    /// Save bracket from chart button.
    ChartBracketSave(ChartId, AnnotationId),
    /// Toggle SL from chart button.
    ChartBracketToggleSL(ChartId, AnnotationId),
    /// Cancel bracket from chart button.
    ChartBracketCancel(ChartId, AnnotationId),
    /// Cancel SL from chart button.
    ChartBracketCancelSL(ChartId, AnnotationId),
    /// Toggle the pin state on the symbol whose bracket this chart
    /// annotation belongs to. Added in Slice 4 to wire the PinToggle
    /// decorator click into `OrderIntentAppMsg::TogglePin`.
    ChartBracketTogglePin(ChartId, AnnotationId),

    // -- Bracket context menu --
    /// Right-click on a bracket leg — show context menu.
    ChartBracketContextMenu(
        ChartId,
        u64,
        midas_chart::widget::order_bracket::LegRole,
        f32,
        f32,
    ),
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
}

/// Classify messages the `wait_for_idle` tracker should NOT treat as
/// input activity: `Tick` fires constantly, market-data updates fire
/// per broker tick, etc. Everything else is considered "real" work.
#[cfg(feature = "dev_harness")]
fn is_tick_rate_message(msg: &Message) -> bool {
    use crate::ticker_state::TickerMsg;
    matches!(
        msg,
        Message::Tick
            | Message::Ticker(_, TickerMsg::UpdateMarketData { .. })
            | Message::RefreshMarketData
            | Message::MarketSnapshotLoaded(..)
    )
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

        let open_task = open_task.map(Message::MainWindowOpened);

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

        let level_store = LevelStore::from_config(&config.levels);

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
            // For each symbol in LevelStore, inject its levels into the
            // corresponding TickerState. Redb data (existing levels in
            // TickerState) takes priority — only inject if TickerState
            // has no levels yet.
            for (ticker, stored_levels) in level_store.all_levels() {
                let sym_key = crate::annotation_store::SymbolKey::new(ticker);
                let ts = tickers
                    .entry(sym_key.clone())
                    .or_insert_with(|| crate::ticker_state::TickerState::new(sym_key.clone()));
                if ts.levels().is_empty() && !stored_levels.is_empty() {
                    ts.inject_levels(stored_levels.to_vec());
                    tracing::debug!(
                        "ticker-state: imported {} level(s) for {ticker} from TOML config",
                        stored_levels.len()
                    );
                }
            }

            // Flush all migrated states to redb so subsequent startups
            // skip migration.
            for (sym, state) in &tickers {
                ticker_persist.upsert(sym.clone(), state.clone());
            }
        }

        // Start the broker engine with TestBroker defaults.
        let broker_bridge = {
            let broker_config = midas_broker::BrokerConfig::default();
            let handle = midas_broker::start_broker_engine(broker_config);
            let bridge = Arc::new(crate::broker_bridge::BrokerBridge::new(
                handle,
                "Test Broker",
            ));
            tracing::info!("Broker engine started (Test Broker, data_source=test)");
            Some(bridge)
        };

        let mut app = Self {
            charts,
            workspace,
            status_message,
            show_frame_overlay: false,
            config_path,
            config_dirty: false,
            last_config_save: Instant::now(),
            current_time,
            main_window: Some(main_id),
            floating_charts: HashMap::new(),
            window_position: config.window.x.zip(config.window.y),
            window_size: initial_size,
            monitor_size: None,
            level_store,
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
            resizing_account_column: None,
            resizing_account_history_column: None,
            resizing_account_recents_column: None,
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
            broker_bridge: broker_bridge.clone(),
            broker_connection_display: "Disconnected".to_string(),
            snapped_this_session: std::collections::HashSet::new(),
            anchor_seed_toasts_shown: std::collections::HashSet::new(),
            gatr_undo_slots: std::collections::HashMap::new(),
            chart_views: crate::chart_view::ChartViewStore::default(),
            thumbnail_store: crate::thumbnail_store::ThumbnailStore::default(),
            thumbnail_data: crate::thumbnail_data::ThumbnailDataStore::default(),
            tickers,
            ticker_persist,
            ticker_dispatch_active: false,
        };

        // Register broker bridge in provider registry.
        if let Some(ref bridge) = app.broker_bridge {
            app.providers.register_order_broker(bridge.clone());
            app.providers.set_active_broker(Some(0));
        }

        // Connect to broker (TestBroker auto-connects, but the command
        // ensures the engine state machine transitions properly).
        if let Some(ref bridge) = app.broker_bridge {
            if let Err(e) = bridge.connect() {
                tracing::warn!("Failed to send initial Connect: {e}");
            }
        }

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

        let startup_task = if load_tasks.is_empty() {
            Task::batch([open_task, watchlist_task, thumbnail_task])
        } else {
            load_tasks.push(open_task);
            load_tasks.push(watchlist_task);
            load_tasks.push(thumbnail_task);
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
    /// Levels are no longer restored per-chart — they live in `LevelStore`.
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
        }
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
    /// This bridges `LevelStore` generation changes to the existing per-chart
    /// `DirtyFlags.levels` counter that the GPU renderer depends on.
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
            .get(&new_upper)
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
    fn propagate_symbol_change(&mut self, source_id: ChartId, new_symbol: &str) -> Task<Message> {
        use crate::link::find_link_targets;

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
            if let Some(chart) = self.charts.get_mut(&id) {
                chart.gatr_hover = false;
            }
            tasks.push(self.load_symbol_for_chart(id, new_symbol));
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
    fn build_provider_registry(config: &AppConfig) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
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

    /// Core async data loader. Calls the active provider and wraps the
    /// result in the message variant produced by `make_msg`.
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
        let provider = match self.providers.active_data_provider() {
            Some(p) => p,
            None => return Task::none(),
        };
        let gen = self
            .charts
            .get(&chart_id)
            .map(|c| c.load_generation)
            .unwrap_or(0);
        let symbol = symbol.to_uppercase();
        let requested_symbol = symbol.clone();
        let days = Self::days_for_timeframe(tf);
        Task::perform(
            async move { provider.get_candles(&symbol, tf, days).await },
            move |result| {
                make_msg(
                    chart_id,
                    requested_symbol,
                    gen,
                    result.map(Arc::new).map_err(|e| e.to_string()),
                )
            },
        )
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

    /// Load a market data snapshot for a symbol from the active data provider.
    fn load_market_snapshot(&self, symbol: &str) -> Task<Message> {
        let provider = match self.providers.active_data_provider() {
            Some(p) => p,
            None => return Task::none(),
        };
        let sym = symbol.to_uppercase();
        let sym_clone = sym.clone();
        Task::perform(
            async move {
                provider
                    .get_candles(&sym, midas_core::Timeframe::D1, 30)
                    .await
            },
            move |result| {
                Message::MarketSnapshotLoaded(sym_clone, result.map_err(|e| e.to_string()))
            },
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

            // -- Chart interaction (viewport, pan, zoom, crosshair,
            //    levels, level editor, toggles, reset, batch) --
            Message::ChartViewportChanged(..)
            | Message::ChartPan(..)
            | Message::ChartZoom(..)
            | Message::ChartZoomY(..)
            | Message::ChartCrosshair(..)
            | Message::ChartCreateLevel(..)
            | Message::ChartDragLevel(..)
            | Message::ChartDragLevelEnd(..)
            | Message::ChartSelectLevel(..)
            | Message::ChartDeselectLevel(..)
            | Message::ChartDeleteSelectedLevel(..)
            | Message::ChartClearAllLevels(..)
            | Message::ChartCancelPlacing(..)
            | Message::PlacingCursorMoved(..)
            | Message::ChartSetTimelineBorderRatio(..)
            | Message::ChartSetVolumeScale(..)
            | Message::ChartRightClickLevel(..)
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
            | Message::AccountOrdersColumnResizeStart(..)
            | Message::AccountOrdersColumnResizing(..)
            | Message::AccountOrdersColumnResizeEnd
            | Message::AccountOrdersOpenColumnSelector(..)
            | Message::AccountOrdersDismissColumnSelector
            | Message::AccountOrdersToggleColumn(..)
            | Message::AccountHistoryColumnResizeStart(..)
            | Message::AccountHistoryColumnResizing(..)
            | Message::AccountHistoryColumnResizeEnd
            | Message::AccountRecentsColumnResizeStart(..)
            | Message::AccountRecentsColumnResizing(..)
            | Message::AccountRecentsColumnResizeEnd
            | Message::AccountPositionsBatch(..) => self.handle_account_panel_msg(message),

            // -- Watchlist --
            Message::AddWatchlist
            | Message::WatchlistTickerInputChanged(..)
            | Message::WatchlistAddTicker(..)
            | Message::WatchlistRemoveTicker(..)
            | Message::WatchlistAdjustFavorite(..)
            | Message::WatchlistTickerPressed(..)
            | Message::WatchlistDragConfirm(..)
            | Message::WatchlistDragCancel
            | Message::DragCursorMoved(..)
            | Message::DragMouseUp
            | Message::WatchlistTickerSelected(..)
            | Message::WatchlistSetSymbolLink(..)
            | Message::WatchlistColumnResizeStart(..)
            | Message::WatchlistColumnResizing(..)
            | Message::WatchlistColumnResizeEnd
            | Message::WatchlistGrid(..) => self.handle_watchlist_msg(message),

            // -- Market data cache --
            Message::MarketSnapshotLoaded(..) | Message::RefreshMarketData => {
                self.handle_market_data_msg(message)
            }

            // -- Chart linking --
            Message::SetSymbolLink(..)
            | Message::SetTimeframeLink(..)
            | Message::FloatingSetSymbolLink(..)
            | Message::FloatingSetTimeframeLink(..)
            | Message::ToggleLinkPicker(..)
            | Message::DismissLinkPicker => self.handle_link_msg(message),

            // -- Window / config / floating --
            Message::ConfigSaved(..)
            | Message::WindowCloseRequested
            | Message::PopOut(..)
            | Message::WindowMoved(..)
            | Message::WindowResized(..)
            | Message::MonitorSizeResult(..)
            | Message::MainWindowOpened(..)
            | Message::FloatingWindowClosed(..) => self.handle_window_config_msg(message),

            // -- G.ATR hover highlight --
            Message::GatrHoverEnter(..) | Message::GatrHoverLeave(..) => {
                self.handle_gatr_hover_msg(message)
            }

            // -- Bracket (drawing tool, drag, action buttons, context menu) --
            Message::ChartCreateBracket(..)
            | Message::ChartDragBracketLeg(..)
            | Message::ChartBracketToggleSL(..)
            | Message::ChartBracketCancelSL(..)
            | Message::ChartBracketTogglePin(..)
            | Message::ChartBracketSave(..)
            | Message::ChartBracketSubmit(..)
            | Message::ChartBracketCancel(..)
            | Message::ChartBracketContextMenu(..)
            | Message::BracketContextCancel(..)
            | Message::BracketContextDismiss => self.handle_bracket_msg(message),

            // -- Broker events --
            Message::BrokerBracketCreated { .. }
            | Message::BrokerBracketStatusChanged { .. }
            | Message::BrokerEventReceived(..)
            | Message::BrokerConnectionChanged(..) => self.handle_broker_msg(message),

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
                self.toasts.clear();
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
}

// View functions (view, view_toolbar, view_content, view_pane_*, view_status_bar)
// are in app/views.rs.
