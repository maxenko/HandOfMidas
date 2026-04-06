//! Application state, Message enum, and update logic.
//!
//! Sub-modules:
//! - `views`: widget tree construction (toolbar, pane grid, status bar)
//! - `persistence`: config build, save, and debounce

mod persistence;
mod views;

use std::collections::HashMap;
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
    CandleBuffer, ChartId, DataProvider, LinkMode, OrderPanelId, Timeframe, WatchlistId,
};

use crate::registry::ProviderRegistry;

use crate::annotation_store::AnnotationStore;
use crate::layout::{LayoutPresetKind, PanelContent, WorkspaceLayout};
use crate::level_store::LevelStore;
use crate::link::{LinkDimension, PickerTarget};
use crate::order_panel::OrderSide;
use crate::watchlist::WatchlistPanel;

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
    pub symbol: String,
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
    /// Dockable order panels keyed by stable OrderPanelId.
    pub order_panels: HashMap<OrderPanelId, crate::order_panel::OrderPanel>,
    /// Links between chart bracket annotations and broker orders,
    /// keyed by the parent (entry) order UUID for O(1) lookup.
    pub order_annotation_links: HashMap<uuid::Uuid, crate::order_panel::OrderAnnotationLink>,
    /// Toast notification message (shown briefly, then auto-dismissed).
    pub toast_message: Option<String>,
    /// When the current toast was created. Used for auto-dismiss timing.
    pub toast_created_at: Option<Instant>,
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
    /// Session-only cache of Draft bracket state, keyed by (panel_id, symbol).
    /// When [X] toggle clears an unsaved bracket, it's moved here.
    /// When BUY/SELL is re-toggled, it's restored from here.
    pub draft_bracket_cache:
        HashMap<(OrderPanelId, String), midas_chart::widget::order_bracket::OrderBracket>,
    /// In-memory market data cache for watchlist columns.
    pub market_cache: crate::market_cache::MarketDataCache,
    /// Bridge to the midas-broker engine. None if engine failed to start.
    pub broker_bridge: Option<Arc<crate::broker_bridge::BrokerBridge>>,
    /// Current broker connection state display string.
    pub broker_connection_display: String,
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
    /// Async data load completed for a chart.
    DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>),

    /// Data loaded during startup restore (does not reset camera).
    DataRestoredFromStartup(ChartId, Result<Arc<CandleBuffer>, String>),

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
    /// Toggle the favorite status of a ticker in a watchlist.
    WatchlistToggleFavorite(WatchlistId, String),
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
    /// Show a toast notification.
    ShowToast(String),
    /// Dismiss the current toast (auto or manual).
    DismissToast,

    // -- Market data cache --
    /// Market data snapshot loaded for a watchlist symbol (D1 candles).
    MarketSnapshotLoaded(String, Result<midas_core::CandleBuffer, String>),
    /// Timer tick to refresh all cached market data.
    RefreshMarketData,

    // -- Window --
    /// Periodic tick for animations and status bar clock.
    Tick,
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

        // Build workspace, charts, watchlists, and order panels from config.
        let (workspace, charts, watchlists, restored_order_panels, status_message);

        if !config.layout_tree.is_empty() {
            // Full topology restoration from layout_tree.
            let (ws, ch, wl, op) = Self::restore_from_layout_tree(
                &config.layout_tree,
                &config.charts,
                &config.watchlists,
                &config.order_panels,
            );
            let n = ch.len() + wl.len() + op.len();
            workspace = ws;
            charts = ch;
            watchlists = wl;
            restored_order_panels = op;
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
            status_message = format!("Restored {n} chart(s) from config");
        } else {
            let (ws, first_id) = WorkspaceLayout::single();
            let mut ch = HashMap::new();
            ch.insert(first_id, Self::make_empty_panel());
            workspace = ws;
            charts = ch;
            watchlists = HashMap::new();
            restored_order_panels = HashMap::new();
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
            crosshair_sync: None,
            providers: Self::build_provider_registry(&config),
            store,
            watchlists,
            cursor_position: iced::Point::ORIGIN,
            pending_drag: None,
            dragging_ticker: None,
            link_picker_open: None,
            resizing_column: None,
            order_panels: restored_order_panels,
            order_annotation_links: HashMap::new(),
            toast_message: None,
            toast_created_at: None,
            bracket_context_menu: None,
            annotation_store: AnnotationStore::new(),
            draft_bracket_cache: HashMap::new(),
            market_cache: crate::market_cache::MarketDataCache::default(),
            broker_bridge: broker_bridge.clone(),
            broker_connection_display: "Disconnected".to_string(),
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

        let startup_task = if load_tasks.is_empty() {
            Task::batch([open_task, watchlist_task])
        } else {
            load_tasks.push(open_task);
            load_tasks.push(watchlist_task);
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
    ) -> (
        WorkspaceLayout,
        HashMap<ChartId, ChartPanel>,
        HashMap<WatchlistId, WatchlistPanel>,
        HashMap<OrderPanelId, crate::order_panel::OrderPanel>,
    ) {
        use crate::layout::PaneState;

        struct RestoreCtx {
            charts: HashMap<ChartId, ChartPanel>,
            watchlists: HashMap<WatchlistId, WatchlistPanel>,
            order_panels: HashMap<OrderPanelId, crate::order_panel::OrderPanel>,
            next_chart_id: u32,
            next_wl_id: u32,
            next_order_id: u32,
            cursor: usize,
        }

        impl RestoreCtx {
            fn parse_node(
                &mut self,
                tree: &[LayoutNode],
                chart_cfgs: &[ChartConfig],
                watchlist_cfgs: &[midas_core::config::WatchlistConfig],
                order_panel_cfgs: &[midas_core::config::OrderPanelConfig],
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
                        let a = self.parse_node(tree, chart_cfgs, watchlist_cfgs, order_panel_cfgs);
                        let b = self.parse_node(tree, chart_cfgs, watchlist_cfgs, order_panel_cfgs);
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
            next_chart_id: 1,
            next_wl_id: 1,
            next_order_id: 1,
            cursor: 0,
        };

        let config = ctx.parse_node(tree, chart_cfgs, watchlist_cfgs, order_panel_cfgs);

        let panes = pane_grid::State::with_configuration(config);
        let first_pane = panes.panes.keys().next().copied();
        let ws = WorkspaceLayout {
            panes,
            focus: first_pane,
            next_chart_id: ctx.next_chart_id,
            next_watchlist_id: ctx.next_wl_id,
            next_order_panel_id: ctx.next_order_id,
        };

        (ws, ctx.charts, ctx.watchlists, ctx.order_panels)
    }

    /// Restore a single chart panel from config.
    ///
    /// Levels are no longer restored per-chart — they live in `LevelStore`.
    fn restore_panel(cfg: &ChartConfig) -> ChartPanel {
        let tf = Timeframe::from_suffix(&cfg.timeframe).unwrap_or(Timeframe::D1);
        let mut panel = Self::make_empty_panel();
        panel.symbol = cfg.symbol.clone();
        panel.symbol_input = cfg.symbol.clone();
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

    // ── Bracket chart toggle helpers ────────────────────────────────

    /// Copy the current bracket annotation into `draft_bracket_cache`.
    ///
    /// Called after every mutation to keep the cache in sync with
    /// the annotation store. Only updates the cache if the panel has
    /// a live bracket annotation.
    fn sync_draft_cache(&mut self, panel_id: OrderPanelId, symbol: &str) {
        let ann_id = match self.order_panels.get(&panel_id) {
            Some(p) => match p.state.bracket_annotation_id {
                Some(id) => id,
                None => return,
            },
            None => return,
        };

        // Find the bracket data in the annotation store.
        let bracket = self
            .annotation_store
            .get(symbol)
            .iter()
            .find(|a| a.id == ann_id)
            .and_then(|a| match &a.kind {
                midas_chart::widget::AnnotationKind::OrderBracket(b) => Some(b.as_ref().clone()),
                _ => None,
            });

        if let Some(bracket) = bracket {
            let key = (panel_id, symbol.to_uppercase());
            self.draft_bracket_cache.insert(key, bracket);
        }
    }

    /// Handle bracket state when an order panel's symbol changes.
    ///
    /// Only Draft brackets participate in the cache/restore cycle.
    /// Pending and Active brackets represent live broker orders and
    /// must NOT be removed on symbol change — they remain in the
    /// AnnotationStore under their original symbol.
    fn handle_order_panel_symbol_change(
        &mut self,
        panel_id: OrderPanelId,
        old_symbol: &str,
        new_symbol: &str,
    ) {
        use midas_chart::widget::order_bracket::*;

        let panel = match self.order_panels.get(&panel_id) {
            Some(p) => p,
            None => return,
        };

        // Nothing to do if bracket mode is off.
        if panel.state.bracket_active.is_none() {
            return;
        }

        let old_upper = old_symbol.to_uppercase();
        let new_upper = new_symbol.to_uppercase();

        // If same symbol (case-insensitive), nothing to do.
        if old_upper == new_upper {
            return;
        }

        // Cache and remove the current Draft bracket for the old symbol.
        if let Some(ann_id) = panel.state.bracket_annotation_id {
            let is_draft = self
                .annotation_store
                .get(&old_upper)
                .iter()
                .find(|a| a.id == ann_id)
                .and_then(|a| match &a.kind {
                    midas_chart::widget::AnnotationKind::OrderBracket(b) => {
                        Some(b.status == BracketStatus::Draft)
                    }
                    _ => None,
                })
                .unwrap_or(false);

            if is_draft {
                // Save to cache before removing (unless already cached
                // by sync_draft_cache).
                self.sync_draft_cache(panel_id, &old_upper);
                let is_saved = self
                    .annotation_store
                    .get(&old_upper)
                    .iter()
                    .find(|a| a.id == ann_id)
                    .and_then(|a| match &a.kind {
                        midas_chart::widget::AnnotationKind::OrderBracket(b) => Some(b.saved),
                        _ => None,
                    })
                    .unwrap_or(false);

                if !is_saved {
                    self.annotation_store.remove(&old_upper, ann_id);
                    self.mark_levels_dirty_for_ticker(&old_upper);
                }
            }
            // Pending/Active brackets stay in AnnotationStore under old
            // symbol — they represent live broker orders.
        }

        // Clear the panel's annotation link (it pointed to old symbol).
        if let Some(p) = self.order_panels.get_mut(&panel_id) {
            p.state.bracket_annotation_id = None;
        }

        // Try to restore a cached Draft bracket for the new symbol.
        let cache_key = (panel_id, new_upper.clone());
        if let Some(cached) = self.draft_bracket_cache.remove(&cache_key) {
            let ann_id = self.annotation_store.add(
                &new_upper,
                midas_chart::widget::AnnotationKind::OrderBracket(Box::new(cached)),
            );
            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                p.state.bracket_annotation_id = Some(ann_id);
            }
            self.sync_draft_cache(panel_id, &new_upper);
        } else {
            // No cached bracket: create a fresh Draft at market price
            // (0.0 if no data yet — will be updated on DataLoaded).
            let bracket_side = match self
                .order_panels
                .get(&panel_id)
                .and_then(|p| p.state.bracket_active)
            {
                Some(crate::order_panel::OrderSide::Buy) => BracketSide::Long,
                Some(crate::order_panel::OrderSide::Sell) => BracketSide::Short,
                None => return,
            };

            let last_price = self
                .market_cache
                .get(&new_upper)
                .and_then(|snap| snap.last_price)
                .unwrap_or(0.0);

            let make_leg = |price: f64| BracketLeg {
                price,
                timestamp: None,
                color: None,
                style: midas_chart::widget::level::LineStyle::Solid,
                line_width: 1.0,
                label: None,
                projected_pnl: None,
                projected_pnl_pct: None,
            };

            let bracket = OrderBracket {
                entry: make_leg(last_price),
                take_profit: None,
                stop_loss: None,
                side: bracket_side,
                status: BracketStatus::Draft,
                quantity: None,
                saved: false,
                filled_qty: None,
                entry_type: midas_chart::widget::order_bracket::EntryType::Market,
                entry_stop_price: None,
                wrong_side_warning: false,
            };

            let ann_id = self.annotation_store.add(
                &new_upper,
                midas_chart::widget::AnnotationKind::OrderBracket(Box::new(bracket)),
            );
            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                p.state.bracket_annotation_id = Some(ann_id);
            }
            self.sync_draft_cache(panel_id, &new_upper);
        }

        self.mark_levels_dirty_for_ticker(&new_upper);
    }

    /// Compute and set `wrong_side_warning` on a bracket based on entry
    /// price vs. market price. Uses bid/ask from market_cache if available,
    /// falls back to last_price.
    fn update_wrong_side_warning(&mut self, ticker: &str, ann_id: AnnotationId) {
        use midas_chart::widget::order_bracket::{BracketSide, EntryType};

        let last_price = self
            .market_cache
            .get(ticker)
            .and_then(|s| s.last_price)
            .unwrap_or(0.0);
        if last_price <= 0.0 {
            return;
        }

        self.annotation_store.update(ticker, ann_id, |ann| {
            if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                b.wrong_side_warning = match (b.entry_type, b.side) {
                    // Limit BUY above market = marketable (warn)
                    (EntryType::Limit, BracketSide::Long) => b.entry.price > last_price,
                    // Limit SELL below market = marketable (warn)
                    (EntryType::Limit, BracketSide::Short) => b.entry.price < last_price,
                    // Stop BUY below market (warn)
                    (EntryType::Stop, BracketSide::Long) => b.entry.price < last_price,
                    // Stop SELL above market (warn)
                    (EntryType::Stop, BracketSide::Short) => b.entry.price > last_price,
                    // StopLimit: warn based on limit price (entry.price)
                    (EntryType::StopLimit, BracketSide::Long) => b.entry.price > last_price,
                    (EntryType::StopLimit, BracketSide::Short) => b.entry.price < last_price,
                    // Market: never warn
                    (EntryType::Market, _) => false,
                };
            }
        });
    }

    /// Handle entry type change for an order panel.
    ///
    /// Updates panel state, updates the bracket annotation's `entry_type`,
    /// and adjusts entry price: switching to Market resets to `last_price`;
    /// switching from Market to Limit/Stop/StopLimit defaults the price
    /// input to `last_price` as a starting point.
    fn handle_set_entry_type(
        &mut self,
        panel_id: OrderPanelId,
        new_type: midas_chart::widget::order_bracket::EntryType,
    ) {
        use midas_chart::widget::order_bracket::EntryType;

        let panel = match self.order_panels.get(&panel_id) {
            Some(p) => p,
            None => return,
        };
        let symbol = panel.state.symbol.clone();
        if symbol.is_empty() {
            return;
        }
        let symbol_upper = symbol.to_uppercase();
        let old_type = panel.state.entry_type;
        let ann_id = panel.state.bracket_annotation_id;

        let last_price = self
            .market_cache
            .get(&symbol_upper)
            .and_then(|snap| snap.last_price)
            .unwrap_or(0.0);

        // Default price inputs when switching away from Market.
        if old_type == EntryType::Market && new_type != EntryType::Market && last_price > 0.0 {
            let side = self
                .order_panels
                .get(&panel_id)
                .map(|p| p.state.side)
                .unwrap_or(crate::order_panel::OrderSide::Buy);
            let is_buy = side == crate::order_panel::OrderSide::Buy;

            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                if p.state.limit_price.is_empty() {
                    p.state.limit_price = format!("{:.2}", last_price);
                }
                if p.state.stop_price.is_empty() {
                    // Stop defaults: +2% for buy, -2% for sell.
                    let stop_default = if is_buy {
                        last_price * 1.02
                    } else {
                        last_price * 0.98
                    };
                    p.state.stop_price = format!("{:.2}", stop_default);
                }
            }
        }

        // Update panel state.
        if let Some(p) = self.order_panels.get_mut(&panel_id) {
            p.state.entry_type = new_type;
        }

        // Update the bracket annotation.
        if let Some(ann_id) = ann_id {
            let limit_price_str = self
                .order_panels
                .get(&panel_id)
                .map(|p| p.state.limit_price.clone())
                .unwrap_or_default();
            let stop_price_str = self
                .order_panels
                .get(&panel_id)
                .map(|p| p.state.stop_price.clone())
                .unwrap_or_default();

            self.annotation_store.update(&symbol_upper, ann_id, |ann| {
                if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                    b.entry_type = new_type;
                    b.wrong_side_warning = false;
                    match new_type {
                        EntryType::Market => {
                            b.entry.price = last_price;
                            b.entry_stop_price = None;
                        }
                        EntryType::Limit => {
                            if let Ok(p) = limit_price_str.parse::<f64>() {
                                b.entry.price = p;
                            }
                            b.entry_stop_price = None;
                        }
                        EntryType::Stop => {
                            if let Ok(p) = stop_price_str.parse::<f64>() {
                                b.entry.price = p;
                            }
                            b.entry_stop_price = None;
                        }
                        EntryType::StopLimit => {
                            if let Ok(p) = limit_price_str.parse::<f64>() {
                                b.entry.price = p;
                            }
                            b.entry_stop_price = stop_price_str.parse::<f64>().ok();
                        }
                    }
                }
            });
            self.sync_draft_cache(panel_id, &symbol_upper);
            self.mark_levels_dirty_for_ticker(&symbol_upper);
        }
    }

    /// Update Draft brackets that have `entry.price == 0.0` for a symbol.
    ///
    /// Called when chart data finishes loading. If a panel activated a
    /// bracket before data was available, the entry price is 0.0; this
    /// patches it to the last close price from the newly loaded data.
    fn update_zero_price_brackets(&mut self, symbol: &str, price: f64) {
        let sym_upper = symbol.to_uppercase();
        let panel_ids: Vec<OrderPanelId> = self
            .order_panels
            .iter()
            .filter(|(_, p)| {
                p.state.bracket_active.is_some()
                    && p.state.symbol.to_uppercase() == sym_upper
                    && p.state.bracket_annotation_id.is_some()
            })
            .map(|(id, _)| *id)
            .collect();

        for pid in panel_ids {
            let ann_id = match self
                .order_panels
                .get(&pid)
                .and_then(|p| p.state.bracket_annotation_id)
            {
                Some(id) => id,
                None => continue,
            };

            // Only update Draft brackets with zero entry price.
            let needs_update = self
                .annotation_store
                .get(&sym_upper)
                .iter()
                .find(|a| a.id == ann_id)
                .and_then(|a| match &a.kind {
                    midas_chart::widget::AnnotationKind::OrderBracket(b) => Some(
                        b.status == midas_chart::widget::order_bracket::BracketStatus::Draft
                            && b.entry.price.abs() < f64::EPSILON,
                    ),
                    _ => None,
                })
                .unwrap_or(false);

            if needs_update {
                self.annotation_store.update(&sym_upper, ann_id, |ann| {
                    if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                        b.entry.price = price;
                    }
                });
                self.sync_draft_cache(pid, &sym_upper);
                self.mark_levels_dirty_for_ticker(&sym_upper);
            }
        }
    }

    /// Handle the BUY/X/SELL bracket toggle for an order panel.
    fn handle_set_bracket_mode(
        &mut self,
        panel_id: OrderPanelId,
        mode: Option<crate::order_panel::OrderSide>,
    ) {
        use midas_chart::widget::order_bracket::*;

        let panel = match self.order_panels.get(&panel_id) {
            Some(p) => p,
            None => return,
        };
        let symbol = panel.state.symbol.clone();
        if symbol.is_empty() {
            return;
        }
        let symbol_upper = symbol.to_uppercase();
        let old_ann_id = panel.state.bracket_annotation_id;
        let panel_entry_type = panel.state.entry_type;
        let panel_limit_price = panel.state.limit_price.clone();
        let panel_stop_price = panel.state.stop_price.clone();

        match mode {
            Some(side) => {
                let bracket_side = match side {
                    crate::order_panel::OrderSide::Buy => BracketSide::Long,
                    crate::order_panel::OrderSide::Sell => BracketSide::Short,
                };

                // If there's already an active bracket for this panel, just flip side.
                if let Some(ann_id) = old_ann_id {
                    self.annotation_store.update(&symbol_upper, ann_id, |ann| {
                        if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) =
                            ann.kind
                        {
                            let old_side = b.side;
                            b.side = bracket_side;
                            // If SL exists and is now on the wrong side, remove it.
                            if old_side != bracket_side {
                                if let Some(ref sl) = b.stop_loss {
                                    let entry = b.entry.price;
                                    let invalid = match bracket_side {
                                        BracketSide::Long => sl.price >= entry,
                                        BracketSide::Short => sl.price <= entry,
                                    };
                                    if invalid {
                                        b.stop_loss = None;
                                    }
                                }
                            }
                        }
                    });
                    if let Some(p) = self.order_panels.get_mut(&panel_id) {
                        p.state.side = side;
                        p.state.bracket_active = Some(side);
                    }
                    self.sync_draft_cache(panel_id, &symbol_upper);
                    self.mark_levels_dirty_for_ticker(&symbol_upper);
                    return;
                }

                // Check for existing saved Draft bracket in AnnotationStore.
                let saved_ann = self
                    .annotation_store
                    .get(&symbol_upper)
                    .iter()
                    .find(|a| {
                        matches!(&a.kind, midas_chart::widget::AnnotationKind::OrderBracket(b)
                            if b.status == BracketStatus::Draft && b.saved)
                    })
                    .map(|a| a.id);

                if let Some(existing_id) = saved_ann {
                    // Re-link to existing saved bracket; update side if needed.
                    self.annotation_store
                        .update(&symbol_upper, existing_id, |ann| {
                            if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) =
                                ann.kind
                            {
                                b.side = bracket_side;
                            }
                        });
                    if let Some(p) = self.order_panels.get_mut(&panel_id) {
                        p.state.side = side;
                        p.state.bracket_active = Some(side);
                        p.state.bracket_annotation_id = Some(existing_id);
                    }
                    self.sync_draft_cache(panel_id, &symbol_upper);
                    self.mark_levels_dirty_for_ticker(&symbol_upper);
                    return;
                }

                // Check draft_bracket_cache for a previously cached bracket.
                let cache_key = (panel_id, symbol_upper.clone());
                let cached = self.draft_bracket_cache.remove(&cache_key);

                let bracket = if let Some(mut b) = cached {
                    b.side = bracket_side;
                    b.entry_type = panel_entry_type;
                    b
                } else {
                    // Create a new bracket at last_price (0.0 if no data yet).
                    let last_price = self
                        .market_cache
                        .get(&symbol_upper)
                        .and_then(|snap| snap.last_price)
                        .unwrap_or(0.0);

                    let make_leg = |price: f64| BracketLeg {
                        price,
                        timestamp: None,
                        color: None,
                        style: midas_chart::widget::level::LineStyle::Solid,
                        line_width: 1.0,
                        label: None,
                        projected_pnl: None,
                        projected_pnl_pct: None,
                    };

                    // Determine entry price based on entry type.
                    let entry_price = match panel_entry_type {
                        EntryType::Market => last_price,
                        EntryType::Limit => panel_limit_price.parse::<f64>().unwrap_or(last_price),
                        EntryType::Stop => panel_stop_price.parse::<f64>().unwrap_or(last_price),
                        EntryType::StopLimit => {
                            panel_limit_price.parse::<f64>().unwrap_or(last_price)
                        }
                    };
                    let entry_stop = if panel_entry_type == EntryType::StopLimit {
                        panel_stop_price.parse::<f64>().ok()
                    } else {
                        None
                    };

                    OrderBracket {
                        entry: make_leg(entry_price),
                        take_profit: None,
                        stop_loss: None,
                        side: bracket_side,
                        status: BracketStatus::Draft,
                        quantity: None,
                        saved: false,
                        filled_qty: None,
                        entry_type: panel_entry_type,
                        entry_stop_price: entry_stop,
                        wrong_side_warning: false,
                    }
                };

                let ann_id = self.annotation_store.add(
                    &symbol_upper,
                    midas_chart::widget::AnnotationKind::OrderBracket(Box::new(bracket)),
                );

                if let Some(p) = self.order_panels.get_mut(&panel_id) {
                    p.state.side = side;
                    p.state.bracket_active = Some(side);
                    p.state.bracket_annotation_id = Some(ann_id);
                }
                self.sync_draft_cache(panel_id, &symbol_upper);
                self.mark_levels_dirty_for_ticker(&symbol_upper);
            }

            None => {
                // [X] toggle: clear unsaved bracket, keep saved ones.
                if let Some(ann_id) = old_ann_id {
                    // Check if the bracket is saved.
                    let is_saved = self
                        .annotation_store
                        .get(&symbol_upper)
                        .iter()
                        .find(|a| a.id == ann_id)
                        .and_then(|a| match &a.kind {
                            midas_chart::widget::AnnotationKind::OrderBracket(b) => Some(b.saved),
                            _ => None,
                        })
                        .unwrap_or(false);

                    if !is_saved {
                        // Cache bracket data before removing.
                        self.sync_draft_cache(panel_id, &symbol_upper);
                        self.annotation_store.remove(&symbol_upper, ann_id);
                        self.mark_levels_dirty_for_ticker(&symbol_upper);
                    }
                    // If saved, leave in AnnotationStore (remains visible).
                }

                if let Some(p) = self.order_panels.get_mut(&panel_id) {
                    p.state.bracket_active = None;
                    p.state.bracket_annotation_id = None;
                }
            }
        }
    }

    /// Set a chart's symbol and asynchronously load data for it.
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

        if let Some(chart) = self.charts.get_mut(&chart_id) {
            chart.symbol = symbol.clone();
            chart.symbol_input = symbol.clone();
            chart.load_state = LoadState::Loading;
            chart.chart_state.dirty.mark_data();
        }

        self.load_chart_async(chart_id, &symbol, tf)
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
        for wid in floating_targets {
            let tf = self
                .floating_charts
                .get(&wid)
                .map(|c| c.timeframe)
                .unwrap_or(Timeframe::D1);
            if let Some(chart) = self.floating_charts.get_mut(&wid) {
                chart.symbol = symbol.clone();
                chart.symbol_input = symbol.clone();
                chart.gatr_hover = false;
                chart.load_state = LoadState::Loading;
                chart.chart_state.dirty.mark_data();
            }
            tasks.push(self.load_floating_chart_async(wid, &symbol, tf));
        }

        // Order panels.
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
            self.handle_order_panel_symbol_change(op_id, &old_sym, &symbol);
            if let Some(panel) = self.order_panels.get_mut(&op_id) {
                panel.state.symbol = symbol.clone();
                panel.state.tp_value.clear();
                panel.state.sl_value.clear();
                panel.state.sl_limit_value.clear();
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
        F: FnOnce(ChartId, Result<Arc<CandleBuffer>, String>) -> Message + Send + 'static,
    {
        let provider = match self.providers.active_data_provider() {
            Some(p) => p,
            None => return Task::none(),
        };
        let symbol = symbol.to_uppercase();
        let days = Self::days_for_timeframe(tf);
        Task::perform(
            async move { provider.get_candles(&symbol, tf, days).await },
            move |result| make_msg(chart_id, result.map(Arc::new).map_err(|e| e.to_string())),
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

    /// Apply loaded candle data to a chart panel, optionally resetting
    /// the camera to show the last 200 candles.
    fn apply_candle_data(chart: &mut ChartPanel, buffer: Arc<CandleBuffer>, reset_camera: bool) {
        chart.data = Some(Arc::clone(&buffer));
        chart.load_state = LoadState::Loaded;
        chart.chart_state.dirty.mark_data();
        if buffer.is_empty() {
            return;
        }
        let len = buffer.len();
        if chart.chart_state.collapse_gaps {
            chart.chart_state.data_time_start = 0.0;
            chart.chart_state.data_time_end = len as f64;
        } else {
            let first_ts = buffer.timestamps[0] as f64;
            let last_ts = buffer.timestamps[len - 1] as f64;
            chart.chart_state.data_time_start = first_ts;
            chart.chart_state.data_time_end = last_ts;
        }
        if reset_camera {
            let visible_count = 200.min(len);
            if chart.chart_state.collapse_gaps {
                let start_idx = (len - visible_count) as f64;
                let end_idx = len as f64 + (visible_count as f64 * 0.05);
                chart.chart_state.camera.time_start = start_idx;
                chart.chart_state.camera.time_end = end_idx;
            } else {
                let last_ts = buffer.timestamps[len - 1] as f64;
                let first_visible_ts = buffer.timestamps[len - visible_count] as f64;
                chart.chart_state.camera.time_start = first_visible_ts;
                chart.chart_state.camera.time_end = last_ts + (last_ts - first_visible_ts) * 0.05;
            }
            let range = (len - visible_count)..len;
            let (low, high) = buffer.price_range(range);
            let padding = (high - low) as f64 * 0.05;
            chart.chart_state.camera.price_low = low as f64 - padding;
            chart.chart_state.camera.price_high = high as f64 + padding;
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
}

// Config persistence (build_config, mark_config_dirty, maybe_save_config,
// flush_config) is in app/persistence.rs.

// ── Update ────────────────────────────────────────────────────────────

impl MidasApp {
    /// Process a message and return any async tasks to execute.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PanelSymbolInputChanged(chart_id, value) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.symbol_input = value;
                }
                Task::none()
            }

            Message::PanelSymbolSubmitted(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.gatr_hover = false;
                }
                let symbol = if let Some(chart) = self.charts.get(&chart_id) {
                    chart.symbol_input.trim().to_uppercase()
                } else {
                    return Task::none();
                };
                if symbol.is_empty() {
                    return Task::none();
                }
                let task = self.load_symbol_for_chart(chart_id, &symbol);
                self.mark_config_dirty();
                let propagate = self.propagate_symbol_change(chart_id, &symbol);
                Task::batch([task, propagate])
            }

            Message::PanelTimeframeSelected(chart_id, tf) => {
                self.focus_chart(chart_id);
                // Get the symbol before mutating, then regenerate data at new tf.
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();

                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.timeframe = tf;
                    chart.gatr_hover = false;
                    chart.chart_state.dirty.mark_camera();
                }

                let mut tasks: Vec<Task<Message>> = Vec::new();
                if !symbol.is_empty() {
                    if let Some(chart) = self.charts.get_mut(&chart_id) {
                        chart.load_state = LoadState::Loading;
                        chart.chart_state.dirty.mark_data();
                    }
                    tasks.push(self.load_chart_async(chart_id, &symbol, tf));
                }

                tasks.push(self.propagate_timeframe_change(chart_id, tf));
                self.mark_config_dirty();
                Task::batch(tasks)
            }

            Message::DataLoaded(chart_id, result) => {
                match result {
                    Ok(buffer) => {
                        let mut loaded_symbol: Option<String> = None;
                        // Grab last close before buffer is moved.
                        let last_close = if buffer.is_empty() {
                            None
                        } else {
                            Some(buffer.closes[buffer.len() - 1] as f64)
                        };
                        // Try docked charts first, then floating charts.
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            let sym = chart.symbol.clone();
                            let count = buffer.len();
                            let tf = chart.timeframe;
                            Self::apply_candle_data(chart, buffer, true);
                            self.status_message =
                                format!("{}: {} candles at {}", sym, count, tf.display_name());
                            loaded_symbol = Some(sym);
                        } else if chart_id == ChartId::new(0) {
                            // Floating chart sentinel: apply to the first
                            // floating chart that is in Loading state.
                            for chart in self.floating_charts.values_mut() {
                                if matches!(chart.load_state, LoadState::Loading) {
                                    loaded_symbol = Some(chart.symbol.clone());
                                    Self::apply_candle_data(chart, buffer, true);
                                    break;
                                }
                            }
                        }

                        // Update zero-price Draft brackets for order panels
                        // on this symbol. When bracket_active is set but data
                        // wasn't loaded yet, entry.price will be 0.0.
                        if let (Some(ref sym), Some(price)) = (&loaded_symbol, last_close) {
                            self.update_zero_price_brackets(sym, price);
                        }

                        // Ensure D1 market snapshot exists for G.ATR display.
                        if let Some(sym) = loaded_symbol {
                            if self.market_cache.get(&sym).is_none() {
                                return self.load_market_snapshot(&sym);
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.load_state = LoadState::Error(e.clone());
                            chart.chart_state.dirty.mark_data();
                        }
                        tracing::warn!(chart = %chart_id, error = %e, "data load failed");
                        self.status_message = format!("Load error: {e}");
                    }
                }
                Task::none()
            }

            Message::DataRestoredFromStartup(chart_id, result) => {
                match result {
                    Ok(buffer) => {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            Self::apply_candle_data(chart, buffer, false);
                        }
                    }
                    Err(e) => {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.load_state = LoadState::Error(e.clone());
                            chart.chart_state.dirty.mark_data();
                        }
                        tracing::warn!(
                            chart = %chart_id, error = %e,
                            "startup data restore failed"
                        );
                    }
                }
                Task::none()
            }

            Message::DataProviderSelected(name) => {
                if let Some(idx) = self.providers.find_data_provider_index(&name) {
                    if self.providers.set_active_data(idx) {
                        tracing::info!(provider = %name, "switched data provider");
                        self.mark_config_dirty();
                        let chart_task = self.reload_all_charts();
                        let market_task = self.load_all_watchlist_snapshots();
                        return Task::batch([chart_task, market_task]);
                    }
                }
                Task::none()
            }

            Message::OrderBrokerSelected(name) => {
                let idx = if name == "None" {
                    None
                } else {
                    self.providers.find_broker_index(&name)
                };
                if self.providers.set_active_broker(idx) {
                    tracing::info!(broker = %name, "switched order broker");
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::AddChart => {
                if let Some(focused) = self.workspace.focus {
                    if let Some((new_id, _new_pane)) =
                        self.workspace.split(pane_grid::Axis::Vertical, focused)
                    {
                        self.charts.insert(new_id, Self::make_empty_panel());
                        self.status_message = format!("Added {new_id}");
                        self.mark_config_dirty();
                    }
                }
                Task::none()
            }

            Message::CloseChart(id) => {
                if let Some(pane) = self.workspace.find_pane(id) {
                    if let Some(PanelContent::Chart(closed_id)) = self.workspace.close(pane) {
                        self.charts.remove(&closed_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Docked(pid), _)) if pid == closed_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {closed_id}");
                        return self.flush_config();
                    }
                }
                Task::none()
            }

            Message::ActivateChart(id) => {
                if let Some(pane) = self.workspace.find_pane(id) {
                    self.workspace.set_focus(pane);
                }
                Task::none()
            }

            Message::LayoutPreset(preset) => {
                self.link_picker_open = None;
                let new_ids = self.workspace.apply_preset(&preset);
                for id in &new_ids {
                    self.charts
                        .entry(*id)
                        .or_insert_with(Self::make_empty_panel);
                }
                let active_ids: std::collections::HashSet<ChartId> =
                    self.workspace.chart_ids().into_iter().collect();
                self.charts.retain(|id, _| active_ids.contains(id));
                // Clean up orphaned watchlist panels (presets create chart-only layouts).
                let active_wl_ids: std::collections::HashSet<WatchlistId> = self
                    .workspace
                    .panes
                    .panes
                    .values()
                    .filter_map(|s| match &s.content {
                        PanelContent::Watchlist(id) => Some(*id),
                        _ => None,
                    })
                    .collect();
                self.watchlists.retain(|id, _| active_wl_ids.contains(id));
                // Clean up orphaned order panels (presets create chart-only layouts).
                self.order_panels.clear();
                self.mark_config_dirty();
                Task::none()
            }

            Message::PaneFocused(pane) => {
                self.workspace.set_focus(pane);
                Task::none()
            }

            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.workspace.panes.resize(split, ratio);
                self.mark_config_dirty();
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Picked { pane }) => {
                self.workspace.set_focus(pane);
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Dropped { pane, target }) => {
                self.pending_drag = None;
                self.dragging_ticker = None;
                self.workspace.panes.drop(pane, target);
                self.mark_config_dirty();
                Task::none()
            }

            Message::PaneDragged(_) => {
                self.pending_drag = None;
                self.dragging_ticker = None;
                Task::none()
            }

            Message::PaneSplit(axis, pane) => {
                if let Some((new_id, _new_pane)) = self.workspace.split(axis, pane) {
                    self.charts.insert(new_id, Self::make_empty_panel());
                    self.status_message = format!("Split pane, added {new_id}");
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::PaneClose(pane) => {
                match self.workspace.close(pane) {
                    Some(PanelContent::Chart(closed_id)) => {
                        self.charts.remove(&closed_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Docked(pid), _)) if pid == closed_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {closed_id}");
                    }
                    Some(PanelContent::Watchlist(wl_id)) => {
                        self.watchlists.remove(&wl_id);
                        self.status_message = format!("Closed {wl_id}");
                    }
                    Some(PanelContent::Order(order_id)) => {
                        self.order_panels.remove(&order_id);
                        if matches!(self.link_picker_open, Some((PickerTarget::Order(pid), _)) if pid == order_id)
                        {
                            self.link_picker_open = None;
                        }
                        self.status_message = format!("Closed {order_id}");
                    }
                    None => return Task::none(),
                }
                self.flush_config()
            }

            Message::ChartViewportChanged(chart_id, old_w, old_h, new_w, new_h) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    if old_w > 0 && old_h > 0 {
                        let w_ratio = new_w as f64 / old_w as f64;
                        let h_ratio = new_h as f64 / old_h as f64;

                        // Horizontal: anchor right edge, expand/contract left.
                        let time_range = cam.time_end - cam.time_start;
                        cam.time_start = cam.time_end - time_range * w_ratio;

                        // Vertical: anchor center, expand/contract both edges.
                        let price_center = (cam.price_high + cam.price_low) / 2.0;
                        let half_range = (cam.price_high - cam.price_low) / 2.0 * h_ratio;
                        cam.price_high = price_center + half_range;
                        cam.price_low = price_center - half_range;
                    }
                    // Update canonical viewport so the snapshot matches
                    // actual bounds on the next frame.
                    cam.viewport_width = new_w;
                    cam.viewport_height = new_h;
                    // Clear crosshair during resize so it doesn't linger.
                    chart.chart_state.crosshair.force_hide();
                    #[allow(deprecated)]
                    {
                        chart.chart_state.crosshair_pos = None;
                    }
                    chart.chart_state.dirty.mark_camera();
                    chart.chart_state.dirty.mark_crosshair();
                }
                Task::none()
            }

            Message::ChartPan(chart_id, dx, dy) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart
                        .chart_state
                        .apply_action(&midas_chart::ChartAction::Pan { dx, dy });
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ChartZoom(chart_id, pivot_time, factor) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    // pivot_time is already in data-space (converted from pixel
                    // in the widget using the camera with correct viewport).
                    let left_dt = pivot_time - cam.time_start;
                    let right_dt = cam.time_end - pivot_time;
                    cam.time_start = pivot_time - left_dt / factor;
                    cam.time_end = pivot_time + right_dt / factor;
                    chart.chart_state.dirty.mark_camera();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ChartZoomY(chart_id, pivot_price, factor) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    // pivot_price is already in data-space (converted from pixel
                    // in the widget using the camera with correct viewport).
                    let up_dp = cam.price_high - pivot_price;
                    let down_dp = pivot_price - cam.price_low;
                    cam.price_high = pivot_price + up_dp / factor;
                    cam.price_low = pivot_price - down_dp / factor;
                    chart.chart_state.dirty.mark_camera();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ChartCrosshair(chart_id, pos) => {
                // Only focus on crosshair set (user actively interacting),
                // not on clear (housekeeping from release/leave). ButtonReleased
                // is delivered to ALL shader widgets, so inactive charts emit
                // ClearCrosshair — focusing on None would steal focus back.
                if pos.is_some() {
                    self.focus_chart(chart_id);
                }
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    match pos {
                        Some((x, y)) => chart.chart_state.crosshair.set_pos(x, y),
                        None => chart.chart_state.crosshair.force_hide(),
                    }
                    #[allow(deprecated)]
                    {
                        chart.chart_state.crosshair_pos = pos;
                    }
                    chart.chart_state.dirty.mark_crosshair();
                }
                // Cross-chart crosshair sync: compute snapped timestamp + price.
                match pos {
                    Some((x, y)) => {
                        if let Some(chart) = self.charts.get(&chart_id) {
                            if let Some(ref data) = chart.data {
                                let cam = &chart.chart_state.camera;
                                let ts = if chart.chart_state.collapse_gaps {
                                    let idx_f = cam.x_to_time(x);
                                    let idx = (idx_f.round().max(0.0) as usize)
                                        .min(data.len().saturating_sub(1));
                                    data.timestamps[idx]
                                } else {
                                    let cursor_time = cam.x_to_time(x);
                                    let idx = data.find_index_by_time(cursor_time as i64);
                                    data.timestamps[idx]
                                };
                                let price = cam.y_to_price(y);
                                self.crosshair_sync =
                                    Some((chart_id, ts, price, chart.symbol.clone()));
                            }
                        }
                    }
                    None => {
                        // Clear sync only if this chart was the source.
                        if self
                            .crosshair_sync
                            .as_ref()
                            .is_some_and(|(src, _, _, _)| *src == chart_id)
                        {
                            self.crosshair_sync = None;
                        }
                    }
                }
                Task::none()
            }

            Message::ChartCreateLevel(chart_id, price) => {
                self.focus_chart(chart_id);
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ref ticker) = ticker {
                    tracing::info!(
                        "CreateLevel: ticker={ticker:?}, price={price}, store_count_before={}",
                        self.level_store.levels_for(ticker).len()
                    );
                }
                if let Some(ticker) = ticker {
                    // Price is already snapped by the interaction layer.
                    let level_id = self.level_store.alloc_id();
                    self.level_store.add_level(
                        &ticker,
                        midas_chart::levels::HorizontalLevel {
                            id: level_id,
                            price,
                            color: [0.85, 0.85, 0.85, 0.8],
                            line_width: 1.0,
                            label: None,
                            icon: midas_chart::LevelIcon::None,
                            locked: false,
                        },
                    );
                    tracing::info!(
                        "CreateLevel: added id={level_id}, store_count_after={}",
                        self.level_store.levels_for(&ticker).len()
                    );
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.level_placing = false;
                    self.placing_preview = None;
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::ChartDragLevel(chart_id, level_id, new_price) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.price = new_price;
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::ChartSelectLevel(chart_id, level_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.selected_level = Some(AnnotationId(level_id));
                    chart.chart_state.dirty.mark_levels();
                }
                Task::none()
            }

            Message::ChartDeselectLevel(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.selected_level = None;
                    chart.chart_state.dirty.mark_levels();
                }
                Task::none()
            }

            Message::ChartDeleteSelectedLevel(chart_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let (Some(ticker), Some(chart)) = (ticker, self.charts.get_mut(&chart_id)) {
                    if let Some(sel_id) = chart.chart_state.selected_level {
                        let is_locked = self
                            .level_store
                            .levels_for(&ticker)
                            .iter()
                            .any(|l| l.id == sel_id.0 && l.locked);
                        if !is_locked {
                            chart.chart_state.selected_level = None;
                            self.level_store.remove_level(&ticker, sel_id.0);
                            self.mark_levels_dirty_for_ticker(&ticker);
                            self.mark_config_dirty();
                        }
                    }
                }
                Task::none()
            }

            Message::ChartClearAllLevels(chart_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    self.level_store.clear_levels(&ticker);
                    self.mark_levels_dirty_for_ticker(&ticker);
                    // Clear selection and editor on all charts for this ticker.
                    for chart in self.charts.values_mut() {
                        if chart.symbol == ticker {
                            chart.chart_state.selected_level = None;
                            chart.editing_level_id = None;
                            chart.editing_level_screen_pos = None;
                        }
                    }
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::ChartCancelPlacing(_chart_id) => {
                self.level_placing = false;
                self.placing_preview = None;
                Task::none()
            }

            Message::PlacingCursorMoved(chart_id, price) => {
                if let Some(ticker) = self.chart_ticker(chart_id) {
                    self.placing_preview = Some((chart_id, ticker.to_owned(), price));
                }
                Task::none()
            }

            Message::ChartSetTimelineBorderRatio(chart_id, ratio) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.timeline_border_ratio = ratio as f32;
                    chart.chart_state.dirty.mark_data();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ChartSetVolumeScale(chart_id, scale) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.volume_scale = scale as f32;
                    chart.chart_state.dirty.mark_data();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ChartRightClickLevel(chart_id, level_id, x, y) => {
                self.focus_chart(chart_id);
                // Read level price from the store (not per-chart state).
                let price_str = self
                    .level_store
                    .find_level(level_id)
                    .map(|(_, l)| midas_chart::format_price(l.price));
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.editing_level_id = Some(level_id);
                    chart.editing_level_screen_pos = Some((x, y));
                    if let Some(ps) = price_str {
                        chart.level_editor_price_input = ps;
                    }
                    chart.chart_state.selected_level = Some(AnnotationId(level_id));
                    chart.chart_state.dirty.mark_levels();
                }
                Task::none()
            }

            Message::ChartCloseLevelEditor(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.editing_level_id = None;
                    chart.editing_level_screen_pos = None;
                }
                Task::none()
            }

            Message::ChartDeleteLevel(chart_id, level_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    let is_locked = self
                        .level_store
                        .levels_for(&ticker)
                        .iter()
                        .any(|l| l.id == level_id && l.locked);
                    if !is_locked {
                        self.level_store.remove_level(&ticker, level_id);
                        self.mark_levels_dirty_for_ticker(&ticker);
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.editing_level_id = None;
                            chart.editing_level_screen_pos = None;
                        }
                        self.mark_config_dirty();
                    }
                }
                Task::none()
            }

            Message::LevelEditorPriceChanged(chart_id, level_id, text) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.level_editor_price_input = text.clone();
                }
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Ok(price) = text.parse::<f64>() {
                        if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                            level.price = price;
                        }
                        self.mark_levels_dirty_for_ticker(&ticker);
                        self.mark_config_dirty();
                    }
                }
                Task::none()
            }

            Message::LevelEditorPriceStep(chart_id, level_id, delta) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.price += delta;
                        let price_str = midas_chart::format_price(level.price);
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.level_editor_price_input = price_str;
                        }
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorLabelChanged(chart_id, level_id, label_text) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.label = if label_text.is_empty() {
                            None
                        } else {
                            Some(label_text)
                        };
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorColorChanged(chart_id, level_id, color) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.color = color;
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorThicknessChanged(chart_id, level_id, thickness) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.line_width = thickness;
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorIconChanged(chart_id, level_id, icon) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.icon = icon;
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::LevelEditorToggleLock(chart_id, level_id) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    if let Some(level) = self.level_store.find_level_mut(&ticker, level_id) {
                        level.locked = !level.locked;
                    }
                    self.mark_levels_dirty_for_ticker(&ticker);
                    self.mark_config_dirty();
                }
                Task::none()
            }

            Message::DrawingPanelCreateLevel(chart_id) => {
                self.focus_chart(chart_id);
                self.level_placing = !self.level_placing;
                if !self.level_placing {
                    self.placing_preview = None;
                }
                Task::none()
            }

            Message::ToggleCollapseGaps(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let was_collapsed = chart.chart_state.collapse_gaps;
                    chart.chart_state.collapse_gaps = !was_collapsed;

                    if let Some(ref data) = chart.data {
                        let len = data.len();
                        if len > 0 {
                            let cam = &mut chart.chart_state.camera;
                            if !was_collapsed {
                                // Switching ON: convert camera from time-space to
                                // index-space so pan/zoom operate uniformly.
                                let start_idx =
                                    data.find_index_by_time(cam.time_start as i64) as f64;
                                let end_idx =
                                    data.find_index_by_time(cam.time_end as i64) as f64 + 1.0;
                                cam.time_start = start_idx;
                                cam.time_end = end_idx;
                                chart.chart_state.data_time_start = 0.0;
                                chart.chart_state.data_time_end = len as f64;
                            } else {
                                // Switching OFF: convert camera from index-space
                                // back to time-space.
                                let si =
                                    (cam.time_start.round() as usize).min(len.saturating_sub(1));
                                let ei = (cam.time_end.round() as usize).min(len.saturating_sub(1));
                                cam.time_start = data.timestamps[si] as f64;
                                cam.time_end = data.timestamps[ei] as f64;
                                chart.chart_state.data_time_start = data.timestamps[0] as f64;
                                chart.chart_state.data_time_end = data.timestamps[len - 1] as f64;
                            }
                        }
                    }
                    chart.chart_state.dirty.mark_camera();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ToggleVolumeProfile(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.show_volume_profile = !chart.chart_state.show_volume_profile;
                    chart.chart_state.dirty.mark_data();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ToggleLevels(chart_id) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.show_levels = !chart.chart_state.show_levels;
                    chart.chart_state.dirty.mark_levels();
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ResetChart(chart_id) => {
                self.focus_chart(chart_id);
                // Reload data at current timeframe to reset camera to default view.
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                let tf = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.timeframe)
                    .unwrap_or(midas_core::Timeframe::D1);
                self.mark_config_dirty();
                if !symbol.is_empty() {
                    if let Some(chart) = self.charts.get_mut(&chart_id) {
                        chart.load_state = LoadState::Loading;
                        chart.chart_state.dirty.mark_data();
                    }
                    return self.load_chart_async(chart_id, &symbol, tf);
                }
                Task::none()
            }

            Message::ChartBatch(msgs) => {
                let tasks: Vec<_> = msgs.into_iter().map(|msg| self.update(msg)).collect();
                Task::batch(tasks)
            }

            Message::KeyPressed(key) => self.handle_key_press(key),

            Message::ConfigSaved(result) => {
                match result {
                    Ok(()) => {}
                    Err(ref e) => {
                        tracing::warn!("Config save failed: {e}");
                        self.status_message = format!("Config save failed: {e}");
                        // Re-mark dirty so the next tick retries the save.
                        self.config_dirty = true;
                    }
                }
                Task::none()
            }

            Message::WindowCloseRequested => {
                if let Some(ref bridge) = self.broker_bridge {
                    let _ = bridge.shutdown();
                }
                self.flush_config()
            }

            Message::PopOut(pane) => {
                if let Some(pane_state) = self.workspace.panes.get(pane) {
                    if let Some(chart_id) = pane_state.chart_id() {
                        if let Some(chart) = self.charts.get(&chart_id) {
                            let floating_chart = chart.clone();
                            let title = if floating_chart.symbol.is_empty() {
                                "Hand of Midas".to_string()
                            } else {
                                format!(
                                    "{} - {}",
                                    floating_chart.symbol,
                                    floating_chart.timeframe.display_name()
                                )
                            };
                            let (win_id, open_task) = window::open(window::Settings {
                                size: iced::Size::new(800.0, 500.0),
                                ..window::Settings::default()
                            });
                            self.floating_charts.insert(win_id, floating_chart);
                            self.status_message = format!("Popped out {title} to new window");
                            self.mark_config_dirty();
                            return open_task.map(|_id| Message::Tick);
                        }
                    }
                }
                Task::none()
            }

            Message::WindowMoved(x, y) => {
                self.window_position = Some((x, y));
                self.mark_config_dirty();
                // Re-query monitor size (window may have moved to a different monitor).
                if let Some(id) = self.main_window {
                    return window::monitor_size(id).map(Message::MonitorSizeResult);
                }
                Task::none()
            }

            Message::WindowResized(w, h) => {
                self.window_size = (w, h);
                self.mark_config_dirty();
                Task::none()
            }

            Message::MonitorSizeResult(size) => {
                if let Some(s) = size {
                    self.monitor_size = Some((s.width as u32, s.height as u32));
                    self.mark_config_dirty();
                }
                Task::none()
            }

            // -- Watchlist --
            Message::AddWatchlist => {
                if let Some(focused) = self.workspace.focus {
                    let wl_id = self.workspace.next_watchlist_id();
                    if let Some((chart_id, new_pane)) =
                        self.workspace.split(pane_grid::Axis::Vertical, focused)
                    {
                        // split() always creates a chart pane — replace it with a watchlist.
                        if let Some(state) = self.workspace.panes.get_mut(new_pane) {
                            state.content = PanelContent::Watchlist(wl_id);
                        }
                        // Remove the chart entry that split() created.
                        self.charts.remove(&chart_id);
                        self.watchlists
                            .insert(wl_id, WatchlistPanel::new(wl_id, "Watchlist".into()));
                        self.status_message = format!("Added {wl_id}");
                        return self.flush_config();
                    }
                }
                Task::none()
            }

            // -- Dockable order panel --
            Message::AddOrderPanel => {
                if let Some(focused) = self.workspace.focus {
                    let op_id = self.workspace.next_order_panel_id();
                    if let Some((chart_id, new_pane)) =
                        self.workspace.split(pane_grid::Axis::Vertical, focused)
                    {
                        // split() always creates a chart pane — replace it with an order panel.
                        if let Some(state) = self.workspace.panes.get_mut(new_pane) {
                            state.content = PanelContent::Order(op_id);
                        }
                        // Remove the chart entry that split() created.
                        self.charts.remove(&chart_id);
                        let symbol = self
                            .active_chart_id()
                            .and_then(|id| self.charts.get(&id))
                            .map(|p| p.symbol.clone())
                            .unwrap_or_default();
                        self.order_panels
                            .insert(op_id, crate::order_panel::OrderPanel::new(op_id, symbol));
                        self.status_message = format!("Added {op_id}");
                        return self.flush_config();
                    }
                }
                Task::none()
            }

            Message::OrderPanelMsg(panel_id, action) => {
                use crate::order_panel::OrderPanelAction;

                // SetBracketMode needs broader access to self (annotation_store, cache).
                if let OrderPanelAction::SetBracketMode(mode) = action {
                    self.handle_set_bracket_mode(panel_id, mode);
                    return Task::none();
                }

                // SetEntryType needs annotation_store + market_cache access.
                if let OrderPanelAction::SetEntryType(new_type) = action {
                    self.handle_set_entry_type(panel_id, new_type);
                    return Task::none();
                }

                // ConfirmYes needs broader access to self (broker_bridge, market_cache),
                // so handle it outside the panel borrow.
                if matches!(action, OrderPanelAction::ConfirmYes) {
                    let panel = match self.order_panels.get(&panel_id) {
                        Some(p) => p,
                        None => return Task::none(),
                    };
                    let state = &panel.state;

                    // Get last_price from market_cache (authoritative source).
                    let last_price = self
                        .market_cache
                        .get(&state.symbol)
                        .and_then(|snap| snap.last_price);

                    let last_price = match last_price {
                        Some(p) => p,
                        None => {
                            if let Some(p) = self.order_panels.get_mut(&panel_id) {
                                p.state.errors =
                                    vec![("price".into(), "Market data not loaded".into())];
                                p.state.showing_confirmation = false;
                            }
                            return Task::none();
                        }
                    };

                    // Resolve TP/SL prices from panel inputs.
                    let tp_price = if state.tp_enabled {
                        state.tp_value.parse::<f64>().ok().map(|val| {
                            crate::order_panel::resolve_price(
                                state.tp_mode,
                                val,
                                last_price,
                                state.side,
                                true,
                            )
                        })
                    } else {
                        None
                    };
                    let sl_price = if state.sl_enabled {
                        state.sl_value.parse::<f64>().ok().map(|val| {
                            crate::order_panel::resolve_price(
                                state.sl_mode,
                                val,
                                last_price,
                                state.side,
                                false,
                            )
                        })
                    } else {
                        None
                    };

                    let action = match state.side {
                        OrderSide::Buy => midas_chart::widget::order_bracket::BracketSide::Long,
                        OrderSide::Sell => midas_chart::widget::order_bracket::BracketSide::Short,
                    };
                    let quantity: f64 = state.quantity.parse().unwrap_or(100.0);
                    let symbol = state.symbol.clone();
                    let side = state.side;

                    tracing::info!(
                        "Order confirmed: {} {} {} (TP: {}, SL: {})",
                        match side {
                            OrderSide::Buy => "BUY",
                            OrderSide::Sell => "SELL",
                        },
                        quantity,
                        symbol,
                        tp_price.is_some(),
                        sl_price.is_some(),
                    );

                    // Send to broker engine.
                    if let Some(ref bridge) = self.broker_bridge {
                        let broker_params = midas_core::broker::BracketParams {
                            symbol: symbol.clone(),
                            con_id: None,
                            sec_type: midas_core::SecurityType::Stock,
                            exchange: "SMART".to_string(),
                            currency: "USD".to_string(),
                            action: match side {
                                OrderSide::Buy => midas_core::broker::OrderAction::Buy,
                                OrderSide::Sell => midas_core::broker::OrderAction::Sell,
                            },
                            quantity,
                            outside_rth: false,
                            take_profit: tp_price.map(|p| midas_core::broker::TakeProfitParams {
                                price: p,
                                tif: None,
                            }),
                            stop_loss: sl_price.map(|p| midas_core::broker::StopLossParams {
                                stop_price: p,
                                limit_price: None,
                                tif: None,
                            }),
                            reference_price: Some(last_price),
                            strategy: None,
                            tags: Vec::new(),
                            entry_kind: midas_core::EntryKind::Market,
                            entry_price: None,
                            entry_stop_price: None,
                        };
                        match bridge.create_bracket(broker_params) {
                            Ok(()) => {
                                tracing::info!(
                                    "CreateBracket sent to broker engine for {}",
                                    symbol
                                );
                            }
                            Err(e) => {
                                tracing::error!("Failed to send bracket to broker: {e}");
                                self.toast_message = Some(format!("Broker error: {e}"));
                                self.toast_created_at = Some(Instant::now());
                            }
                        }
                    } else {
                        tracing::warn!("No broker bridge: CreateBracket for {} not sent", symbol,);
                    }

                    self.status_message = format!(
                        "Order submitted: {} {} {}",
                        match side {
                            OrderSide::Buy => "BUY",
                            OrderSide::Sell => "SELL",
                        },
                        quantity,
                        symbol,
                    );

                    // Clear confirmation on the panel.
                    if let Some(p) = self.order_panels.get_mut(&panel_id) {
                        p.state.showing_confirmation = false;
                    }

                    // Create chart annotation via self-message so the bracket is
                    // visible on all charts displaying this symbol.
                    return self.update(Message::BrokerBracketCreated {
                        parent_id: uuid::Uuid::now_v7(),
                        take_profit_id: tp_price.map(|_| uuid::Uuid::now_v7()),
                        stop_loss_id: sl_price.map(|_| uuid::Uuid::now_v7()),
                        symbol,
                        action,
                        quantity,
                        entry_price: Some(last_price),
                        tp_price,
                        sl_price,
                    });
                }

                if let Some(panel) = self.order_panels.get_mut(&panel_id) {
                    match action {
                        OrderPanelAction::SetSide(side) => panel.state.side = side,
                        OrderPanelAction::SetQuantity(qty) => panel.state.quantity = qty,
                        OrderPanelAction::ToggleTp(enabled) => panel.state.tp_enabled = enabled,
                        OrderPanelAction::SetTpMode(mode) => panel.state.tp_mode = mode,
                        OrderPanelAction::SetTpValue(val) => panel.state.tp_value = val,
                        OrderPanelAction::ToggleSl(enabled) => panel.state.sl_enabled = enabled,
                        OrderPanelAction::SetSlMode(mode) => panel.state.sl_mode = mode,
                        OrderPanelAction::SetSlValue(val) => panel.state.sl_value = val,
                        OrderPanelAction::SetSlType(sl_type) => panel.state.sl_type = sl_type,
                        OrderPanelAction::SetSlLimit(val) => panel.state.sl_limit_value = val,
                        OrderPanelAction::Submit => {
                            // Sync last_price from market_cache so validate_panel
                            // can check TP/SL direction against current price.
                            panel.state.last_price = self
                                .market_cache
                                .get(&panel.state.symbol)
                                .and_then(|snap| snap.last_price);
                            let errors = crate::order_panel::validate_panel(&panel.state);
                            let valid = errors.is_empty();
                            panel.state.errors = errors;
                            if valid {
                                panel.state.showing_confirmation = true;
                            }
                        }
                        OrderPanelAction::ConfirmNo => {
                            panel.state.showing_confirmation = false;
                        }
                        OrderPanelAction::Dismiss => {
                            panel.state.showing_confirmation = false;
                        }
                        OrderPanelAction::SetLimitPrice(val) => {
                            panel.state.limit_price = val;
                        }
                        OrderPanelAction::SetStopPrice(val) => {
                            panel.state.stop_price = val;
                        }
                        OrderPanelAction::ConfirmYes
                        | OrderPanelAction::SetBracketMode(_)
                        | OrderPanelAction::SetEntryType(_) => {
                            // Handled above (outside the panel borrow).
                            unreachable!();
                        }
                    }
                }
                Task::none()
            }

            Message::WatchlistTickerInputChanged(wl_id, value) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.add_ticker_input = value;
                }
                Task::none()
            }

            Message::WatchlistAddTicker(wl_id) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    let input = wl.add_ticker_input.clone();
                    if wl.add_ticker(&input) {
                        wl.add_ticker_input.clear();
                        // Always load fresh data — don't rely on potentially stale cache.
                        let symbol = input.trim().to_uppercase();
                        let task = self.load_market_snapshot(&symbol);
                        return Task::batch([self.flush_config(), task]);
                    }
                }
                Task::none()
            }

            Message::WatchlistRemoveTicker(wl_id, symbol) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    if wl.selected_symbol.as_deref() == Some(symbol.as_str()) {
                        wl.selected_symbol = None;
                    }
                    wl.remove_ticker(&symbol);
                    // Remove from cache if no watchlist still has this symbol.
                    let symbol_upper = symbol.to_uppercase();
                    let still_used = self
                        .watchlists
                        .values()
                        .any(|wl| wl.has_ticker(&symbol_upper));
                    if !still_used {
                        self.market_cache.remove(&symbol_upper);
                    }
                    return self.flush_config();
                }
                Task::none()
            }

            Message::WatchlistToggleFavorite(wl_id, symbol) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.toggle_favorite(&symbol);
                    return self.flush_config();
                }
                Task::none()
            }

            Message::WatchlistTickerPressed(wl_id, symbol) => {
                self.pending_drag = Some(PendingDragState {
                    symbol: symbol.clone(),
                    wl_id,
                });
                // Fire confirmation after 250ms hold.
                Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    },
                    move |_| Message::WatchlistDragConfirm(symbol),
                )
            }

            Message::WatchlistDragConfirm(symbol) => {
                // Only promote if the pending drag matches (hasn't been cancelled).
                if self.pending_drag.as_ref().map(|p| &p.symbol) == Some(&symbol) {
                    self.pending_drag = None;
                    self.dragging_ticker = Some(DragTickerState {
                        symbol,
                        cursor_pos: self.cursor_position,
                    });
                }
                Task::none()
            }

            Message::WatchlistDragCancel => {
                self.pending_drag = None;
                self.dragging_ticker = None;
                Task::none()
            }

            Message::DragCursorMoved(pos) => {
                self.cursor_position = pos;
                if let Some(ref mut drag) = self.dragging_ticker {
                    drag.cursor_pos = pos;
                }
                Task::none()
            }

            Message::DragMouseUp => {
                // If still in pending state (released before 250ms), treat as
                // a regular ticker click — select the ticker in the watchlist.
                if let Some(pending) = self.pending_drag.take() {
                    return self.update(Message::WatchlistTickerSelected(
                        pending.wl_id,
                        pending.symbol,
                    ));
                }

                let drag = match self.dragging_ticker.take() {
                    Some(d) => d,
                    None => return Task::none(),
                };

                // Hit-test: find the chart pane under the cursor.
                // The pane grid sits below the toolbar (~32px) and above the
                // status bar (~26px). We compute pane regions relative to the
                // pane grid origin, then offset to window coordinates.
                const TOOLBAR_H: f32 = 32.0;
                const STATUS_H: f32 = 26.0;
                let (win_w, win_h) = self.window_size;
                let grid_w = win_w as f32;
                let grid_h = (win_h as f32 - TOOLBAR_H - STATUS_H).max(1.0);

                let regions = self.workspace.panes.layout().pane_regions(
                    1.0, // spacing
                    0.0, // min_size
                    iced::Size::new(grid_w, grid_h),
                );

                let cursor = drag.cursor_pos;
                // Translate cursor from window-space to pane-grid-space.
                let local_x = cursor.x;
                let local_y = cursor.y - TOOLBAR_H;

                for (pane, rect) in &regions {
                    if local_x >= rect.x
                        && local_x <= rect.x + rect.width
                        && local_y >= rect.y
                        && local_y <= rect.y + rect.height
                    {
                        if let Some(ps) = self.workspace.panes.get(*pane) {
                            if let Some(chart_id) = ps.chart_id() {
                                self.workspace.set_focus(*pane);
                                let load = self.load_symbol_for_chart(chart_id, &drag.symbol);
                                let propagate =
                                    self.propagate_symbol_change(chart_id, &drag.symbol);
                                self.mark_config_dirty();
                                return Task::batch([load, propagate]);
                            }
                        }
                    }
                }

                // Mouse-up was not on a chart pane — cancel drag.
                Task::none()
            }

            Message::WatchlistTickerSelected(wl_id, symbol) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.selected_symbol = Some(symbol.clone());
                }

                // Propagate to linked charts using watchlist's own link mode.
                use crate::link::find_link_targets;
                let wl_link = self
                    .watchlists
                    .get(&wl_id)
                    .map(|wl| wl.symbol_link)
                    .unwrap_or(LinkMode::Unlinked);

                let mut tasks = Vec::new();

                let targets: Vec<ChartId> = find_link_targets(
                    wl_link,
                    self.charts.iter().map(|(id, p)| (*id, p.symbol_link)),
                );
                for id in targets {
                    tasks.push(self.load_symbol_for_chart(id, &symbol));
                }

                let floating_targets: Vec<window::Id> = find_link_targets(
                    wl_link,
                    self.floating_charts
                        .iter()
                        .map(|(wid, p)| (*wid, p.symbol_link)),
                );
                for wid in floating_targets {
                    let tf = self
                        .floating_charts
                        .get(&wid)
                        .map(|c| c.timeframe)
                        .unwrap_or(Timeframe::D1);
                    if let Some(chart) = self.floating_charts.get_mut(&wid) {
                        chart.symbol = symbol.clone();
                        chart.symbol_input = symbol.clone();
                        chart.load_state = LoadState::Loading;
                        chart.chart_state.dirty.mark_data();
                    }
                    tasks.push(self.load_floating_chart_async(wid, &symbol, tf));
                }

                // Propagate to order panels.
                let order_targets: Vec<OrderPanelId> = find_link_targets(
                    wl_link,
                    self.order_panels.iter().map(|(id, p)| (*id, p.symbol_link)),
                );
                for op_id in order_targets {
                    let old_sym = self
                        .order_panels
                        .get(&op_id)
                        .map(|p| p.state.symbol.clone())
                        .unwrap_or_default();
                    self.handle_order_panel_symbol_change(op_id, &old_sym, &symbol);
                    if let Some(panel) = self.order_panels.get_mut(&op_id) {
                        panel.state.symbol = symbol.clone();
                        panel.state.tp_value.clear();
                        panel.state.sl_value.clear();
                        panel.state.sl_limit_value.clear();
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

            Message::WatchlistSetSymbolLink(wl_id, mode) => {
                self.link_picker_open = None;
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    wl.symbol_link = mode;
                }
                self.flush_config()
            }

            Message::OrderPanelSetSymbolLink(op_id, mode) => {
                self.link_picker_open = None;
                if let Some(panel) = self.order_panels.get_mut(&op_id) {
                    panel.symbol_link = mode;
                }
                self.flush_config()
            }

            Message::WatchlistColumnResizeStart(wl_id, col, _) => {
                let ids = crate::watchlist::WATCHLIST_COLUMN_ORDER;
                if col >= ids.len() {
                    return Task::none();
                }
                let width = self
                    .watchlists
                    .get(&wl_id)
                    .map(|wl| wl.grid_state.column_width(ids[col]))
                    .unwrap_or(70.0);
                // start_x is NaN until the first on_move event provides cursor position.
                self.resizing_column = Some((wl_id, col, f32::NAN, width));
                Task::none()
            }

            Message::WatchlistColumnResizing(current_x) => {
                if let Some((wl_id, col, ref mut start_x, orig_w)) = self.resizing_column {
                    if start_x.is_nan() {
                        *start_x = current_x;
                    }
                    let delta = current_x - *start_x;
                    let new_w = (orig_w + delta).max(20.0);
                    let ids = crate::watchlist::WATCHLIST_COLUMN_ORDER;
                    if col < ids.len() {
                        if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                            wl.grid_state.set_column_width(ids[col], new_w, 20.0, None);
                        }
                    }
                }
                Task::none()
            }

            Message::WatchlistColumnResizeEnd => {
                self.resizing_column = None;
                self.flush_config()
            }

            Message::WatchlistGrid(wl_id, grid_msg) => {
                if let Some(wl) = self.watchlists.get_mut(&wl_id) {
                    match grid_msg {
                        midas_grid::GridMessage::SortToggled(col_id) => {
                            // Per-column default direction: numeric columns start Descending.
                            let default_dir = match col_id {
                                crate::watchlist::COL_PRICE
                                | crate::watchlist::COL_CHANGE
                                | crate::watchlist::COL_GATR => {
                                    midas_grid::SortDirection::Descending
                                }
                                _ => midas_grid::SortDirection::Ascending,
                            };
                            wl.grid_state.toggle_sort(col_id, default_dir);
                        }
                        midas_grid::GridMessage::RowSelected(_) => {
                            // Row clicks emit WatchlistTickerSelected directly from
                            // the view (with the correct symbol from sorted order).
                            // This arm is reserved for Phase 2 keyboard navigation.
                        }
                    }
                }
                Task::none()
            }

            // -- Market data cache --
            Message::MarketSnapshotLoaded(symbol, Ok(buffer)) => {
                // Insert if any watchlist or chart still references this symbol.
                let in_watchlist = self.watchlists.values().any(|wl| wl.has_ticker(&symbol));
                let in_chart = self.charts.values().any(|c| c.symbol == symbol)
                    || self.floating_charts.values().any(|c| c.symbol == symbol);
                if in_watchlist || in_chart {
                    let snapshot = crate::market_cache::snapshot_from_candles(&buffer);
                    self.market_cache.insert(symbol.clone(), snapshot);
                }

                // Sync draft bracket entry prices for Market-type panels tracking this symbol.
                // Non-Market brackets preserve user-specified entry prices.
                let symbol_upper = symbol.to_uppercase();
                if let Some(new_price) = self
                    .market_cache
                    .get(&symbol_upper)
                    .and_then(|s| s.last_price)
                {
                    let mut dirty = false;
                    for panel in self.order_panels.values() {
                        if panel.state.bracket_active.is_some()
                            && panel.state.symbol.to_uppercase() == symbol_upper
                            && panel.state.entry_type
                                == midas_chart::widget::order_bracket::EntryType::Market
                        {
                            if let Some(ann_id) = panel.state.bracket_annotation_id {
                                let should_update = self
                                    .annotation_store
                                    .get(&symbol_upper)
                                    .iter()
                                    .find(|a| a.id == ann_id)
                                    .and_then(|a| match &a.kind {
                                        midas_chart::widget::AnnotationKind::OrderBracket(b) => {
                                            Some((new_price - b.entry.price).abs() >= 0.01)
                                        }
                                        _ => None,
                                    })
                                    .unwrap_or(false);

                                if should_update {
                                    self.annotation_store.update(&symbol_upper, ann_id, |ann| {
                                        if let midas_chart::widget::AnnotationKind::OrderBracket(
                                            ref mut b,
                                        ) = ann.kind
                                        {
                                            b.entry.price = new_price;
                                        }
                                    });
                                    dirty = true;
                                }
                            }
                        }
                    }
                    if dirty {
                        self.mark_levels_dirty_for_ticker(&symbol_upper);
                    }
                }

                Task::none()
            }
            Message::MarketSnapshotLoaded(_symbol, Err(e)) => {
                tracing::warn!("Failed to load market snapshot: {e}");
                Task::none()
            }
            Message::RefreshMarketData => {
                // Refresh all watchlist symbols, not just cached ones.
                // This retries any symbols whose initial load failed.
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

            // -- Chart linking --
            Message::SetSymbolLink(chart_id, mode) => {
                self.link_picker_open = None;
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.symbol_link = mode;
                }
                // Adopt group symbol when joining a link group.
                let mut adopt_task = Task::none();
                let siblings = || {
                    self.charts
                        .iter()
                        .filter(|(id, _)| **id != chart_id)
                        .map(|(_, panel)| panel)
                        .chain(self.floating_charts.values())
                };
                if let LinkMode::Color(color) = mode {
                    let group_symbol = siblings()
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(c) if c == color)
                                && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone());
                    if let Some(symbol) = group_symbol {
                        adopt_task = self.load_symbol_for_chart(chart_id, &symbol);
                    }
                } else if mode == LinkMode::ListenAll {
                    let group_symbol = siblings()
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(_)) && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone());
                    if let Some(symbol) = group_symbol {
                        adopt_task = self.load_symbol_for_chart(chart_id, &symbol);
                    }
                }
                self.mark_config_dirty();
                adopt_task
            }

            Message::SetTimeframeLink(chart_id, mode) => {
                self.link_picker_open = None;
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.timeframe_link = mode;
                }
                // Adopt group timeframe when joining a link group.
                let siblings = || {
                    self.charts
                        .iter()
                        .filter(|(id, _)| **id != chart_id)
                        .map(|(_, panel)| panel)
                        .chain(self.floating_charts.values())
                };
                let group_tf = if let LinkMode::Color(color) = mode {
                    siblings()
                        .find(|p| matches!(p.timeframe_link, LinkMode::Color(c) if c == color))
                        .map(|p| p.timeframe)
                } else if mode == LinkMode::ListenAll {
                    siblings()
                        .find(|p| matches!(p.timeframe_link, LinkMode::Color(_)))
                        .map(|p| p.timeframe)
                } else {
                    None
                };
                if let Some(tf) = group_tf {
                    let symbol = self
                        .charts
                        .get(&chart_id)
                        .map(|c| c.symbol.clone())
                        .unwrap_or_default();
                    if let Some(chart) = self.charts.get_mut(&chart_id) {
                        chart.timeframe = tf;
                    }
                    if !symbol.is_empty() {
                        if let Some(chart) = self.charts.get_mut(&chart_id) {
                            chart.load_state = LoadState::Loading;
                            chart.chart_state.dirty.mark_data();
                        }
                        self.mark_config_dirty();
                        return self.load_chart_async(chart_id, &symbol, tf);
                    }
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::FloatingSetSymbolLink(wid, mode) => {
                self.link_picker_open = None;
                if let Some(chart) = self.floating_charts.get_mut(&wid) {
                    chart.symbol_link = mode;
                }
                // Adopt group symbol when joining a link group.
                let siblings = || {
                    self.charts.values().chain(
                        self.floating_charts
                            .iter()
                            .filter(|(id, _)| **id != wid)
                            .map(|(_, p)| p),
                    )
                };
                let group_symbol = if let LinkMode::Color(color) = mode {
                    siblings()
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(c) if c == color)
                                && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone())
                } else if mode == LinkMode::ListenAll {
                    siblings()
                        .find(|p| {
                            matches!(p.symbol_link, LinkMode::Color(_)) && !p.symbol.is_empty()
                        })
                        .map(|p| p.symbol.clone())
                } else {
                    None
                };
                if let Some(symbol) = group_symbol {
                    let tf = self
                        .floating_charts
                        .get(&wid)
                        .map(|c| c.timeframe)
                        .unwrap_or(Timeframe::D1);
                    if let Some(chart) = self.floating_charts.get_mut(&wid) {
                        chart.symbol = symbol.clone();
                        chart.symbol_input = symbol.clone();
                        chart.load_state = LoadState::Loading;
                        chart.chart_state.dirty.mark_data();
                    }
                    return self.load_floating_chart_async(wid, &symbol, tf);
                }
                // Floating charts are not persisted — no mark_config_dirty needed.
                Task::none()
            }

            Message::FloatingSetTimeframeLink(wid, mode) => {
                self.link_picker_open = None;
                if let Some(chart) = self.floating_charts.get_mut(&wid) {
                    chart.timeframe_link = mode;
                }
                // Adopt group timeframe when joining a link group.
                let siblings = || {
                    self.charts.values().chain(
                        self.floating_charts
                            .iter()
                            .filter(|(id, _)| **id != wid)
                            .map(|(_, p)| p),
                    )
                };
                let group_tf = if let LinkMode::Color(color) = mode {
                    siblings()
                        .find(|p| matches!(p.timeframe_link, LinkMode::Color(c) if c == color))
                        .map(|p| p.timeframe)
                } else if mode == LinkMode::ListenAll {
                    siblings()
                        .find(|p| matches!(p.timeframe_link, LinkMode::Color(_)))
                        .map(|p| p.timeframe)
                } else {
                    None
                };
                if let Some(tf) = group_tf {
                    let symbol = self
                        .floating_charts
                        .get(&wid)
                        .map(|c| c.symbol.clone())
                        .unwrap_or_default();
                    if let Some(chart) = self.floating_charts.get_mut(&wid) {
                        chart.timeframe = tf;
                    }
                    if !symbol.is_empty() {
                        if let Some(chart) = self.floating_charts.get_mut(&wid) {
                            chart.load_state = LoadState::Loading;
                            chart.chart_state.dirty.mark_data();
                        }
                        return self.load_floating_chart_async(wid, &symbol, tf);
                    }
                }
                // Floating charts are not persisted — no mark_config_dirty needed.
                Task::none()
            }

            Message::ToggleLinkPicker(target, dim) => {
                if self.link_picker_open == Some((target, dim)) {
                    self.link_picker_open = None;
                } else {
                    self.link_picker_open = Some((target, dim));
                }
                Task::none()
            }

            Message::DismissLinkPicker => {
                self.link_picker_open = None;
                Task::none()
            }

            Message::MainWindowOpened(id) => {
                tracing::info!("Main window opened: {id}");
                self.main_window = Some(id);
                // Query the monitor size for config persistence.
                window::monitor_size(id).map(Message::MonitorSizeResult)
            }

            Message::FloatingWindowClosed(id) => {
                if matches!(self.link_picker_open, Some((PickerTarget::Floating(wid), _)) if wid == id)
                {
                    self.link_picker_open = None;
                }
                if let Some(chart) = self.floating_charts.remove(&id) {
                    tracing::info!("Floating window closed for {}", chart.symbol);
                }
                // If the main window was closed, exit the application.
                if self.main_window == Some(id) {
                    return self.flush_config().chain(iced::exit());
                }
                Task::none()
            }

            // -- G.ATR hover highlight --
            Message::GatrHoverEnter(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.gatr_hover = true;
                    chart.chart_state.dirty.mark_candles();
                } else {
                    // Floating windows use ChartId(0) which isn't in self.charts.
                    for fc in self.floating_charts.values_mut() {
                        fc.gatr_hover = true;
                        fc.chart_state.dirty.mark_candles();
                    }
                }
                Task::none()
            }
            Message::GatrHoverLeave(chart_id) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.gatr_hover = false;
                    chart.chart_state.dirty.mark_candles();
                } else {
                    for fc in self.floating_charts.values_mut() {
                        fc.gatr_hover = false;
                        fc.chart_state.dirty.mark_candles();
                    }
                }
                Task::none()
            }

            // -- Bracket creation from drawing tool --
            Message::ChartCreateBracket(chart_id, entry, tp, sl, side) => {
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ticker) = ticker {
                    use midas_chart::widget::level::LineStyle;
                    use midas_chart::widget::order_bracket::*;

                    let make_leg = |price: f64| BracketLeg {
                        price,
                        timestamp: None,
                        color: None,
                        style: LineStyle::Solid,
                        line_width: 1.5,
                        label: None,
                        projected_pnl: None,
                        projected_pnl_pct: None,
                    };
                    // Drawing tool doesn't capture quantity — use None so labels
                    // show clean "▲ 185.50" instead of misleading "▲ 185.50  0sh".
                    let bracket = OrderBracket {
                        entry: make_leg(entry),
                        take_profit: Some(make_leg(tp)),
                        stop_loss: Some(make_leg(sl)),
                        side,
                        status: BracketStatus::Draft,
                        quantity: None,
                        saved: false,
                        filled_qty: None,
                        entry_type: midas_chart::widget::order_bracket::EntryType::Market,
                        entry_stop_price: None,
                        wrong_side_warning: false,
                    };
                    let annotation_id = self.annotation_store.add(
                        &ticker,
                        midas_chart::widget::AnnotationKind::OrderBracket(Box::new(bracket)),
                    );
                    self.mark_levels_dirty_for_ticker(&ticker);
                    tracing::info!(
                        "Bracket drawn on chart: {annotation_id} for {ticker} \
                         ({side:?} entry={entry:.2} tp={tp:.2} sl={sl:.2})"
                    );
                    self.status_message = format!(
                        "Bracket placed on {ticker} ({side:?} E={entry:.2} TP={tp:.2} SL={sl:.2})"
                    );
                }
                Task::none()
            }

            // -- Bracket drag --
            Message::ChartDragBracketLeg(chart_id, annotation_id, leg, new_price) => {
                use midas_chart::widget::order_bracket::LegRole;

                let ann_id = AnnotationId(annotation_id);

                // Resolve the ticker for this chart so we can look up the
                // annotation in the per-symbol store.
                let ticker = self.chart_ticker(chart_id).map(str::to_owned);
                if let Some(ref ticker) = ticker {
                    let updated = self.annotation_store.update(ticker, ann_id, |ann| {
                        if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut bracket) =
                            ann.kind
                        {
                            let entry_price = bracket.entry.price;
                            let qty = bracket.quantity.unwrap_or(0.0);
                            let sign = match bracket.side {
                                midas_chart::widget::order_bracket::BracketSide::Long => 1.0,
                                midas_chart::widget::order_bracket::BracketSide::Short => -1.0,
                            };
                            match leg {
                                LegRole::Entry => {
                                    bracket.entry.price = new_price;
                                }
                                LegRole::TakeProfit => {
                                    if let Some(ref mut tp) = bracket.take_profit {
                                        tp.price = new_price;
                                        // Recompute projected P&L if entry has filled.
                                        if bracket.status
                                            == midas_chart::widget::order_bracket::BracketStatus::Active
                                        {
                                            tp.projected_pnl =
                                                Some(sign * (new_price - entry_price) * qty);
                                            tp.projected_pnl_pct =
                                                if entry_price.abs() > f64::EPSILON {
                                                    Some(
                                                        sign * (new_price - entry_price)
                                                            / entry_price
                                                            * 100.0,
                                                    )
                                                } else {
                                                    None
                                                };
                                        }
                                    }
                                }
                                LegRole::StopLoss => {
                                    if let Some(ref mut sl) = bracket.stop_loss {
                                        sl.price = new_price;
                                        if bracket.status
                                            == midas_chart::widget::order_bracket::BracketStatus::Active
                                        {
                                            sl.projected_pnl =
                                                Some(sign * (new_price - entry_price) * qty);
                                            sl.projected_pnl_pct =
                                                if entry_price.abs() > f64::EPSILON {
                                                    Some(
                                                        sign * (new_price - entry_price)
                                                            / entry_price
                                                            * 100.0,
                                                    )
                                                } else {
                                                    None
                                                };
                                        }
                                    }
                                }
                            }
                        }
                    });
                    if updated {
                        tracing::debug!(
                            "Bracket leg drag: chart={chart_id:?} ann={annotation_id} \
                             leg={leg:?} price={new_price:.4}"
                        );
                        // Mark levels dirty on all charts showing this symbol
                        // so the GPU re-renders the bracket lines.
                        self.mark_levels_dirty_for_ticker(ticker);

                        // Entry leg drag: sync panel inputs and compute warnings.
                        if leg == LegRole::Entry {
                            // Find the panel linked to this annotation.
                            let panel_id = self
                                .order_panels
                                .iter()
                                .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
                                .map(|(id, _)| *id);
                            if let Some(pid) = panel_id {
                                if let Some(p) = self.order_panels.get_mut(&pid) {
                                    let price_str = format!("{:.2}", new_price);
                                    match p.state.entry_type {
                                        // StopLimit: drag adjusts limit price only.
                                        // Stop price is only editable via panel
                                        // input (V1 — second drag line deferred).
                                        midas_chart::widget::order_bracket::EntryType::Limit
                                        | midas_chart::widget::order_bracket::EntryType::StopLimit => {
                                            p.state.limit_price = price_str;
                                        }
                                        midas_chart::widget::order_bracket::EntryType::Stop => {
                                            p.state.stop_price = price_str;
                                        }
                                        _ => {}
                                    }
                                }
                                // Compute directional warning.
                                self.update_wrong_side_warning(ticker, ann_id);
                                self.sync_draft_cache(pid, ticker);
                            }
                        }

                        // Send price modification to broker engine.
                        if let Some(ref bridge) = self.broker_bridge {
                            let order_id = self
                                .order_annotation_links
                                .values()
                                .find(|link| link.annotation_id == annotation_id)
                                .and_then(|link| match leg {
                                    LegRole::TakeProfit => link.tp_order_id,
                                    LegRole::StopLoss => link.sl_order_id,
                                    LegRole::Entry => None,
                                });
                            if let Some(order_id) = order_id {
                                if let Err(e) = bridge.modify_bracket_leg(order_id, new_price) {
                                    tracing::error!(
                                        "Failed to send ModifyBracketLeg to broker: {e}"
                                    );
                                } else {
                                    tracing::debug!(
                                        "ModifyBracketLeg sent: order={order_id} \
                                         price={new_price:.4}"
                                    );
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            "Bracket leg drag: annotation {annotation_id} not found \
                             for symbol {ticker}"
                        );
                    }
                }
                Task::none()
            }

            // -- Bracket action buttons (from chart hit zones) --
            Message::ChartBracketToggleSL(chart_id, ann_id) => {
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                if symbol.is_empty() {
                    return Task::none();
                }

                // Read current bracket state.
                let bracket_info = self
                    .annotation_store
                    .get(&symbol)
                    .iter()
                    .find(|a| a.id == ann_id)
                    .and_then(|a| match &a.kind {
                        midas_chart::widget::AnnotationKind::OrderBracket(b) => {
                            Some((b.stop_loss.is_some(), b.entry.price, b.side))
                        }
                        _ => None,
                    });

                let Some((has_sl, entry_price, side)) = bracket_info else {
                    return Task::none();
                };

                // No-op if entry price is zero (no market data).
                if entry_price <= 0.0 && !has_sl {
                    return Task::none();
                }

                self.annotation_store.update(&symbol, ann_id, |ann| {
                    if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                        if b.stop_loss.is_some() {
                            b.stop_loss = None;
                        } else {
                            // Add SL at 2% below (Long) or above (Short).
                            use midas_chart::widget::order_bracket::*;
                            let sl_price = match side {
                                BracketSide::Long => entry_price * 0.98,
                                BracketSide::Short => entry_price * 1.02,
                            };
                            b.stop_loss = Some(BracketLeg {
                                price: sl_price,
                                timestamp: None,
                                color: None,
                                style: midas_chart::widget::level::LineStyle::Solid,
                                line_width: 1.5,
                                label: None,
                                projected_pnl: None,
                                projected_pnl_pct: None,
                            });
                        }
                    }
                });

                // Sync cache for whichever panel owns this bracket.
                let panel_id = self
                    .order_panels
                    .iter()
                    .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
                    .map(|(id, _)| *id);
                if let Some(pid) = panel_id {
                    self.sync_draft_cache(pid, &symbol);
                }
                self.mark_levels_dirty_for_ticker(&symbol);
                Task::none()
            }

            Message::ChartBracketCancelSL(chart_id, ann_id) => {
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                if symbol.is_empty() {
                    return Task::none();
                }

                self.annotation_store.update(&symbol, ann_id, |ann| {
                    if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                        b.stop_loss = None;
                    }
                });

                let panel_id = self
                    .order_panels
                    .iter()
                    .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
                    .map(|(id, _)| *id);
                if let Some(pid) = panel_id {
                    self.sync_draft_cache(pid, &symbol);
                }
                self.mark_levels_dirty_for_ticker(&symbol);
                Task::none()
            }

            Message::ChartBracketSave(_chart_id, ann_id) => {
                // Find the symbol from whichever panel owns this bracket.
                let panel_info = self
                    .order_panels
                    .iter()
                    .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
                    .map(|(id, p)| (*id, p.state.symbol.clone()));

                if let Some((panel_id, symbol)) = panel_info {
                    self.annotation_store.update(&symbol, ann_id, |ann| {
                        if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) =
                            ann.kind
                        {
                            b.saved = true;
                        }
                    });
                    self.sync_draft_cache(panel_id, &symbol);
                    // Re-render so the saved bracket shows brighter alpha.
                    self.mark_levels_dirty_for_ticker(&symbol);
                }
                Task::none()
            }

            Message::ChartBracketSubmit(chart_id, ann_id) => {
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                if symbol.is_empty() {
                    return Task::none();
                }

                // Find which panel owns this bracket.
                let panel_id = self
                    .order_panels
                    .iter()
                    .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
                    .map(|(id, _)| *id);

                // Read bracket data for validation.
                let bracket_data = self
                    .annotation_store
                    .get(&symbol)
                    .iter()
                    .find(|a| a.id == ann_id)
                    .and_then(|a| match &a.kind {
                        midas_chart::widget::AnnotationKind::OrderBracket(b) => {
                            Some(b.as_ref().clone())
                        }
                        _ => None,
                    });

                let Some(bracket) = bracket_data else {
                    return Task::none();
                };

                // Resolve quantity: bracket.quantity > panel.quantity > error.
                let quantity: f64 = if let Some(q) = bracket.quantity {
                    q
                } else if let Some(pid) = panel_id {
                    self.order_panels
                        .get(&pid)
                        .and_then(|p| p.state.quantity.parse::<f64>().ok())
                        .unwrap_or(0.0)
                } else {
                    0.0
                };

                // Validate.
                let errors = crate::order_panel::validate_bracket(&bracket, quantity);
                if !errors.is_empty() {
                    if let Some(pid) = panel_id {
                        if let Some(p) = self.order_panels.get_mut(&pid) {
                            p.state.errors = errors;
                        }
                    }
                    return Task::none();
                }

                // Map chart EntryType → desktop EntryKind for broker params.
                let entry_kind = match bracket.entry_type {
                    midas_chart::widget::order_bracket::EntryType::Market => {
                        midas_core::EntryKind::Market
                    }
                    midas_chart::widget::order_bracket::EntryType::Limit => {
                        midas_core::EntryKind::Limit
                    }
                    midas_chart::widget::order_bracket::EntryType::Stop => {
                        midas_core::EntryKind::Stop
                    }
                    midas_chart::widget::order_bracket::EntryType::StopLimit => {
                        midas_core::EntryKind::StopLimit
                    }
                };

                // Entry type → broker BracketParams field mapping:
                //   Market:    entry_price = None,                entry_stop_price = None
                //   Limit:     entry_price = Some(limit_price),   entry_stop_price = None
                //   Stop:      entry_price = None,                entry_stop_price = Some(stop_price)
                //   StopLimit: entry_price = Some(limit_price),   entry_stop_price = Some(stop_price)
                let (entry_price, entry_stop_price) = match bracket.entry_type {
                    midas_chart::widget::order_bracket::EntryType::Market => (None, None),
                    midas_chart::widget::order_bracket::EntryType::Limit => {
                        (Some(bracket.entry.price), None)
                    }
                    midas_chart::widget::order_bracket::EntryType::Stop => {
                        (None, Some(bracket.entry.price))
                    }
                    midas_chart::widget::order_bracket::EntryType::StopLimit => {
                        (Some(bracket.entry.price), bracket.entry_stop_price)
                    }
                };

                let action = match bracket.side {
                    midas_chart::widget::order_bracket::BracketSide::Long => {
                        midas_core::broker::OrderAction::Buy
                    }
                    midas_chart::widget::order_bracket::BracketSide::Short => {
                        midas_core::broker::OrderAction::Sell
                    }
                };

                // Transition bracket to Pending.
                self.annotation_store.update(&symbol, ann_id, |ann| {
                    if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) = ann.kind {
                        b.status = midas_chart::widget::order_bracket::BracketStatus::Pending;
                    }
                });

                // Send to broker engine.
                if let Some(ref bridge) = self.broker_bridge {
                    let broker_params = midas_core::broker::BracketParams {
                        symbol: symbol.clone(),
                        con_id: None,
                        sec_type: midas_core::SecurityType::Stock,
                        exchange: "SMART".to_string(),
                        currency: "USD".to_string(),
                        action,
                        quantity,
                        outside_rth: false,
                        take_profit: bracket.take_profit.as_ref().map(|tp| {
                            midas_core::broker::TakeProfitParams {
                                price: tp.price,
                                tif: None,
                            }
                        }),
                        stop_loss: bracket.stop_loss.as_ref().map(|sl| {
                            midas_core::broker::StopLossParams {
                                stop_price: sl.price,
                                limit_price: None,
                                tif: None,
                            }
                        }),
                        reference_price: Some(bracket.entry.price),
                        strategy: None,
                        tags: Vec::new(),
                        entry_kind,
                        entry_price,
                        entry_stop_price,
                    };
                    match bridge.create_bracket(broker_params) {
                        Ok(()) => {
                            tracing::info!(
                                "CreateBracket sent: chart={chart_id:?} ann={ann_id} \
                                 symbol={symbol} qty={quantity} type={entry_kind:?}"
                            );
                        }
                        Err(e) => {
                            tracing::error!("Failed to send bracket to broker: {e}");
                            // Revert to Draft on broker rejection.
                            self.annotation_store.update(&symbol, ann_id, |ann| {
                                if let midas_chart::widget::AnnotationKind::OrderBracket(
                                    ref mut b,
                                ) = ann.kind
                                {
                                    b.status =
                                        midas_chart::widget::order_bracket::BracketStatus::Draft;
                                }
                            });
                            if let Some(pid) = panel_id {
                                if let Some(p) = self.order_panels.get_mut(&pid) {
                                    p.state.errors =
                                        vec![("broker".into(), format!("Broker error: {e}"))];
                                }
                            }
                            self.mark_levels_dirty_for_ticker(&symbol);
                            return Task::none();
                        }
                    }
                } else {
                    tracing::info!(
                        "Bracket submitted (no broker bridge): \
                         chart={chart_id:?} ann={ann_id} symbol={symbol} qty={quantity}"
                    );
                }

                // Clear panel bracket ownership.
                if let Some(pid) = panel_id {
                    if let Some(p) = self.order_panels.get_mut(&pid) {
                        p.state.bracket_active = None;
                        p.state.bracket_annotation_id = None;
                    }
                }
                self.mark_levels_dirty_for_ticker(&symbol);
                Task::none()
            }

            Message::ChartBracketCancel(chart_id, ann_id) => {
                let symbol = self
                    .charts
                    .get(&chart_id)
                    .map(|c| c.symbol.clone())
                    .unwrap_or_default();
                if symbol.is_empty() {
                    return Task::none();
                }

                // Find which panel owns this bracket.
                let panel_id = self
                    .order_panels
                    .iter()
                    .find(|(_, p)| p.state.bracket_annotation_id == Some(ann_id))
                    .map(|(id, _)| *id);

                // Remove from annotation store.
                self.annotation_store.remove(&symbol, ann_id);

                // Remove from draft cache.
                if let Some(pid) = panel_id {
                    let cache_key = (pid, symbol.to_uppercase());
                    self.draft_bracket_cache.remove(&cache_key);

                    // Clear panel bracket ownership.
                    if let Some(p) = self.order_panels.get_mut(&pid) {
                        p.state.bracket_active = None;
                        p.state.bracket_annotation_id = None;
                    }
                }
                self.mark_levels_dirty_for_ticker(&symbol);
                Task::none()
            }

            // -- Bracket context menu --
            Message::ChartBracketContextMenu(chart_id, ann_id, leg, x, y) => {
                self.bracket_context_menu = Some((chart_id, ann_id, leg, x, y));
                Task::none()
            }
            Message::BracketContextCancel(parent_id) => {
                self.bracket_context_menu = None;

                // Look up the link but do NOT remove it yet.
                // The link stays alive until the engine confirms cancellation.
                if let Some(link) = self.order_annotation_links.get(&parent_id).cloned() {
                    // Visually mark the annotation as Cancelled immediately.
                    let ann_id = midas_chart::AnnotationId(link.annotation_id);
                    self.annotation_store.update(&link.symbol, ann_id, |ann| {
                        if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut b) =
                            ann.kind
                        {
                            b.status = midas_chart::widget::order_bracket::BracketStatus::Cancelled;
                        }
                    });
                    self.mark_levels_dirty_for_ticker(&link.symbol);
                    tracing::info!("Bracket {parent_id} cancel requested from context menu");

                    // Send cancellation to broker engine.
                    if let Some(ref bridge) = self.broker_bridge {
                        match bridge.cancel_bracket(parent_id) {
                            Ok(()) => {
                                tracing::info!(
                                    "CancelBracket sent to broker engine for {parent_id}"
                                );
                            }
                            Err(e) => {
                                tracing::error!("Failed to send CancelBracket to broker: {e}");
                            }
                        }
                    } else {
                        // No engine — remove link now since there will be no confirmation.
                        self.order_annotation_links.remove(&parent_id);
                    }
                }
                self.toast_message = Some("Bracket cancelled".to_string());
                self.toast_created_at = Some(Instant::now());
                Task::none()
            }
            Message::BracketContextDismiss => {
                self.bracket_context_menu = None;
                Task::none()
            }

            // -- Broker bracket events --
            Message::BrokerBracketCreated {
                parent_id,
                take_profit_id,
                stop_loss_id,
                symbol,
                action,
                quantity,
                entry_price,
                tp_price,
                sl_price,
            } => {
                let entry = entry_price.unwrap_or(0.0);
                let bracket = crate::order_panel::create_bracket_annotation(
                    action, entry, tp_price, sl_price, quantity,
                );

                // Add the annotation to the centralized store for this symbol.
                let annotation_id = self.annotation_store.add(
                    &symbol,
                    midas_chart::widget::AnnotationKind::OrderBracket(Box::new(bracket)),
                );

                // Store the mapping from annotation to broker order IDs.
                let link = crate::order_panel::OrderAnnotationLink {
                    annotation_id: annotation_id.0,
                    parent_order_id: parent_id,
                    tp_order_id: take_profit_id,
                    sl_order_id: stop_loss_id,
                    symbol: symbol.clone(),
                    side: action,
                    quantity,
                    created_at: std::time::Instant::now(),
                };
                self.order_annotation_links
                    .insert(link.parent_order_id, link);

                tracing::info!(
                    "Bracket annotation created: {annotation_id} for {symbol} \
                     (parent={parent_id}, entry={entry:.2})"
                );

                self.status_message =
                    format!("Bracket annotation {annotation_id} created for {symbol}");
                Task::none()
            }

            Message::BrokerEventReceived(boxed_event) => {
                use midas_broker::BrokerEvent;

                match *boxed_event {
                    BrokerEvent::BracketCreated {
                        parent_id,
                        take_profit_id,
                        stop_loss_id,
                        symbol,
                        action,
                        quantity,
                        tp_price,
                        sl_price,
                        reference_price,
                    } => {
                        // Reconcile: find the existing annotation created locally
                        // by matching symbol + side + quantity using cached fields.
                        let side = crate::broker_bridge::translate_action_to_side(&action);

                        let mut candidates: Vec<_> = self
                            .order_annotation_links
                            .iter()
                            .filter(|(_, link)| {
                                link.symbol == symbol
                                    && link.side == side
                                    && (link.quantity - quantity).abs() < 0.01
                            })
                            .collect();
                        candidates.sort_by_key(|(_, link)| link.created_at);

                        let matching_key = candidates.first().map(|(key, _)| **key);

                        if let Some(old_key) = matching_key {
                            if let Some(mut link) = self.order_annotation_links.remove(&old_key) {
                                link.parent_order_id = parent_id;
                                link.tp_order_id = take_profit_id;
                                link.sl_order_id = stop_loss_id;
                                self.order_annotation_links.insert(parent_id, link);
                                tracing::info!(
                                    "Reconciled bracket annotation: provisional \
                                     {old_key} -> engine {parent_id} for {symbol}"
                                );
                            }
                        } else {
                            // No local annotation — create from engine event.
                            let entry_price = reference_price.unwrap_or(0.0);
                            return self.update(Message::BrokerBracketCreated {
                                parent_id,
                                take_profit_id,
                                stop_loss_id,
                                symbol,
                                action: side,
                                quantity,
                                entry_price: Some(entry_price),
                                tp_price,
                                sl_price,
                            });
                        }
                    }
                    BrokerEvent::BracketStatusChanged {
                        parent_id,
                        status,
                        entry_fill_price,
                    } => {
                        use midas_chart::widget::order_bracket::BracketStatus;
                        let chart_status = match status {
                            midas_broker::BracketLifecycleStatus::Submitted => {
                                BracketStatus::Pending
                            }
                            midas_broker::BracketLifecycleStatus::EntryFilled => {
                                BracketStatus::Active
                            }
                            midas_broker::BracketLifecycleStatus::TakeProfitHit => {
                                BracketStatus::Closed
                            }
                            midas_broker::BracketLifecycleStatus::StopLossHit => {
                                BracketStatus::Closed
                            }
                            midas_broker::BracketLifecycleStatus::Cancelled => {
                                BracketStatus::Cancelled
                            }
                            midas_broker::BracketLifecycleStatus::Rejected => {
                                BracketStatus::Cancelled
                            }
                            midas_broker::BracketLifecycleStatus::Error => BracketStatus::Cancelled,
                            midas_broker::BracketLifecycleStatus::Closed => BracketStatus::Closed,
                        };
                        return self.update(Message::BrokerBracketStatusChanged {
                            parent_id,
                            status: chart_status,
                            entry_fill_price,
                        });
                    }
                    BrokerEvent::OrderFilled {
                        order_id,
                        shares,
                        price,
                        commission,
                        ..
                    } => {
                        tracing::info!(
                            "Order filled: {order_id} {shares} shares @ {price:.2} \
                             (commission: {commission:?})"
                        );
                        let msg = format!(
                            "Filled: {shares} @ ${price:.2}{}",
                            commission
                                .map(|c| format!(" (comm ${c:.2})"))
                                .unwrap_or_default()
                        );
                        self.toast_message = Some(msg);
                        self.toast_created_at = Some(Instant::now());
                    }
                    BrokerEvent::OrderRejected { order_id, reason } => {
                        tracing::warn!("Order rejected: {order_id}: {reason}");
                        self.toast_message = Some(format!("Order rejected: {reason}"));
                        self.toast_created_at = Some(Instant::now());
                    }
                    BrokerEvent::OrderCancelled { order_id, reason } => {
                        tracing::info!("Order cancelled: {order_id}: {reason}");
                    }
                    BrokerEvent::Connected { server_version } => {
                        tracing::info!("Broker connected (server v{server_version})");
                        self.status_message = format!("Broker connected (v{server_version})");
                    }
                    BrokerEvent::Disconnected { reason } => {
                        tracing::warn!("Broker disconnected: {reason}");
                        self.status_message = format!("Broker disconnected: {reason}");
                    }
                    BrokerEvent::OrderValidationFailed { message, code } => {
                        tracing::warn!("Order validation failed [{code}]: {message}");
                        self.toast_message = Some(format!("Validation: {message}"));
                        self.toast_created_at = Some(Instant::now());
                    }
                    other => {
                        tracing::trace!("Unhandled broker event: {other:?}");
                    }
                }
                Task::none()
            }

            Message::BrokerBracketStatusChanged {
                parent_id,
                status,
                entry_fill_price,
            } => {
                // Find the annotation link by parent broker order ID.
                if let Some(link) = self.order_annotation_links.get(&parent_id).cloned() {
                    let ann_id = midas_chart::widget::AnnotationId(link.annotation_id);
                    let qty = link.quantity;
                    let updated = self.annotation_store.update(&link.symbol, ann_id, |ann| {
                        if let midas_chart::widget::AnnotationKind::OrderBracket(ref mut bracket) =
                            ann.kind
                        {
                            bracket.status = status;
                            if let Some(fill_price) = entry_fill_price {
                                bracket.entry.price = fill_price;

                                // Compute projected P&L on TP/SL legs.
                                let sign = match bracket.side {
                                    midas_chart::widget::order_bracket::BracketSide::Long => 1.0,
                                    midas_chart::widget::order_bracket::BracketSide::Short => -1.0,
                                };
                                if let Some(ref mut tp) = bracket.take_profit {
                                    let pnl = sign * (tp.price - fill_price) * qty;
                                    tp.projected_pnl = Some(pnl);
                                    tp.projected_pnl_pct = if fill_price.abs() > f64::EPSILON {
                                        Some(sign * (tp.price - fill_price) / fill_price * 100.0)
                                    } else {
                                        None
                                    };
                                }
                                if let Some(ref mut sl) = bracket.stop_loss {
                                    let pnl = sign * (sl.price - fill_price) * qty;
                                    sl.projected_pnl = Some(pnl);
                                    sl.projected_pnl_pct = if fill_price.abs() > f64::EPSILON {
                                        Some(sign * (sl.price - fill_price) / fill_price * 100.0)
                                    } else {
                                        None
                                    };
                                }
                            }
                        }
                    });
                    if updated {
                        tracing::info!(
                            "Bracket {ann_id} status -> {status:?} \
                             (parent={parent_id})"
                        );
                        // Mark charts dirty so GPU re-renders bracket lines.
                        self.mark_levels_dirty_for_ticker(&link.symbol);

                        // Toast notification for significant status changes.
                        use midas_chart::widget::order_bracket::BracketStatus;
                        let toast = match status {
                            BracketStatus::Active => {
                                let price_str = entry_fill_price
                                    .map(|p| format!(" @ ${p:.2}"))
                                    .unwrap_or_default();
                                Some(format!("{} entry filled{price_str}", link.symbol))
                            }
                            BracketStatus::Closed => {
                                Some(format!("{} bracket closed", link.symbol))
                            }
                            BracketStatus::Cancelled => {
                                Some(format!("{} bracket cancelled", link.symbol))
                            }
                            _ => None,
                        };
                        if let Some(msg) = toast {
                            self.toast_message = Some(msg);
                            self.toast_created_at = Some(Instant::now());
                        }

                        // Remove link when engine confirms cancellation (S9).
                        if status == BracketStatus::Cancelled {
                            self.order_annotation_links.remove(&parent_id);
                            tracing::info!(
                                "Annotation link removed for cancelled bracket \
                                 {parent_id}"
                            );
                        }
                    } else {
                        tracing::warn!(
                            "Bracket annotation {ann_id} not found in store for \
                             symbol {} (parent={parent_id})",
                            link.symbol
                        );
                    }
                } else {
                    tracing::warn!("No annotation link found for parent_id={parent_id}");
                }
                Task::none()
            }

            Message::BrokerConnectionChanged(state_str) => {
                self.broker_connection_display = state_str;
                Task::none()
            }

            // -- Toast notifications --
            Message::ShowToast(msg) => {
                self.toast_message = Some(msg);
                self.toast_created_at = Some(Instant::now());
                Task::none()
            }
            Message::DismissToast => {
                self.toast_message = None;
                self.toast_created_at = None;
                Task::none()
            }

            Message::Tick => {
                // Auto-dismiss toast after 4 seconds.
                if let (Some(_toast), Some(created)) = (&self.toast_message, self.toast_created_at)
                {
                    if created.elapsed() > std::time::Duration::from_secs(4) {
                        self.toast_message = None;
                        self.toast_created_at = None;
                    }
                }
                self.maybe_save_config()
            }
        }
    }

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
                self.toast_message = None;
                self.toast_created_at = None;
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
