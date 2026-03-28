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
use midas_core::config::{AppConfig, ChartConfig, LevelConfig};
use midas_core::{ChartId, Timeframe};
use midas_data::CandleBuffer;
use midas_feed::TestDataProvider;

use crate::layout::{LayoutPresetKind, WorkspaceLayout};

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
    /// Deterministic test data generator. Any ticker produces instant data.
    test_data: TestDataProvider,
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
    /// Async CSV data load completed for a chart.
    DataLoaded(ChartId, Result<Arc<CandleBuffer>, String>),

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
    /// Set the volume bar height multiplier for a chart.
    ChartSetVolumeScale(ChartId, f64),

    // -- Gap collapsing --
    /// Toggle session gap collapsing on a chart panel.
    ToggleCollapseGaps(ChartId),
    /// Reset chart to default view (fit all data).
    ResetChart(ChartId),

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
        let base = dirs::config_local_dir()
            .unwrap_or_else(|| PathBuf::from("data"));
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
    fn validate_saved_position(
        wc: &midas_core::config::WindowConfig,
    ) -> window::Position {
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
            size: iced::Size::new(
                config.window.width as f32,
                config.window.height as f32,
            ),
            position: initial_position,
            ..window::Settings::default()
        });

        let open_task = open_task.map(Message::MainWindowOpened);

        // Build workspace and charts from config (or empty defaults).
        let (workspace, charts, status_message) =
            if config.charts.is_empty() {
                let (ws, first_id) = WorkspaceLayout::single();
                let mut charts = HashMap::new();
                charts.insert(first_id, Self::make_empty_panel());
                (ws, charts, "Ready".to_string())
            } else {
                let (mut ws, first_id) = WorkspaceLayout::single();
                let mut charts = HashMap::new();

                // First chart goes into the initial pane.
                let first_cfg = &config.charts[0];
                let mut panel = Self::restore_panel(first_cfg);
                charts.insert(first_id, panel);

                // Additional charts split vertically from the first pane.
                let first_pane = ws.focus.unwrap();
                for chart_cfg in config.charts.iter().skip(1) {
                    panel = Self::restore_panel(chart_cfg);
                    if let Some((new_id, _)) =
                        ws.split(pane_grid::Axis::Vertical, first_pane)
                    {
                        charts.insert(new_id, panel);
                    }
                }
                ws.set_focus(first_pane);

                let n = charts.len();
                (ws, charts, format!("Restored {n} chart(s) from config"))
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
            test_data: TestDataProvider::new(),
        };

        // Auto-load test data for all restored charts that have a symbol.
        let chart_ids: Vec<(ChartId, String, Timeframe)> = app
            .charts
            .iter()
            .filter(|(_, panel)| !panel.symbol.is_empty())
            .map(|(&id, panel)| (id, panel.symbol.clone(), panel.timeframe))
            .collect();
        for (id, symbol, tf) in chart_ids {
            app.load_test_data_for_chart(id, &symbol, tf, false);
        }

        (app, open_task)
    }

    /// Restore a single chart panel from config.
    fn restore_panel(cfg: &ChartConfig) -> ChartPanel {
        let tf = Timeframe::from_suffix(&cfg.timeframe)
            .unwrap_or(Timeframe::D1);
        let mut panel = Self::make_empty_panel();
        panel.symbol = cfg.symbol.clone();
        panel.symbol_input = cfg.symbol.clone();
        panel.timeframe = tf;
        Self::restore_levels(&cfg.levels, &mut panel);
        Self::restore_camera(cfg, &mut panel);
        panel
    }

    /// Restore horizontal levels from config into a chart panel.
    fn restore_levels(level_cfgs: &[LevelConfig], panel: &mut ChartPanel) {
        for level_cfg in level_cfgs {
            let level_id = panel.chart_state.alloc_level_id();
            panel.chart_state.levels.push(
                midas_chart::levels::HorizontalLevel {
                    id: level_id,
                    price: level_cfg.price,
                    color: level_cfg.color,
                    line_width: level_cfg.line_width,
                },
            );
        }
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
        panel.chart_state.volume_scale = chart_cfg.volume_scale;
        // Restore viewport so the first-frame ChartViewportChanged computes
        // the correct ratio (saved viewport → actual pane size) instead of
        // using the dummy 1280×720 from make_empty_panel.
        if let (Some(vw), Some(vh)) =
            (chart_cfg.viewport_width, chart_cfg.viewport_height)
        {
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
        }
    }

    /// Generate test data for a symbol and apply it to a chart panel.
    ///
    /// The [`TestDataProvider`] generates deterministic data instantly for
    /// any ticker string. No file lookup is needed.
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
        }

        self.load_test_data_for_chart(chart_id, &symbol, tf, true);
        Task::none()
    }

    /// Generate test data at the given timeframe and install it in the chart.
    ///
    /// Called from symbol submit, timeframe change, and config restore.
    /// Load test data for a chart and optionally reset its camera.
    ///
    /// When `reset_camera` is `true` (user changed symbol or timeframe), the
    /// camera is positioned to show the last 200 candles.  When `false`
    /// (restoring from config), the camera is left untouched — only
    /// `data_time_start/end` are set for scroll clamping.
    fn load_test_data_for_chart(
        &mut self,
        chart_id: ChartId,
        symbol: &str,
        tf: Timeframe,
        reset_camera: bool,
    ) {
        tracing::debug!("Loading data for chart {chart_id} symbol={symbol} tf={tf}");
        // Choose how many calendar days of data to generate based on
        // timeframe: more for coarser timeframes so the chart isn't empty.
        let days = match tf.as_secs() {
            s if s >= Timeframe::W1.as_secs() => 3650,  // ~10 years
            s if s >= Timeframe::D1.as_secs() => 730,   // ~2 years
            s if s >= Timeframe::H1.as_secs() => 90,    // ~3 months
            s if s >= Timeframe::M15.as_secs() => 30,   // ~1 month
            _ => 10,                                      // <=M5: ~10 days
        };

        let buffer = self.test_data.get_candles(symbol, tf, days);
        let count = buffer.len();
        let buffer = Arc::new(buffer);

        if let Some(chart) = self.charts.get_mut(&chart_id) {
            chart.data = Some(Arc::clone(&buffer));
            chart.load_state = LoadState::Loaded;
            chart.chart_state.dirty.mark_data();

            if !buffer.is_empty() {
                let len = buffer.len();

                // Always set data bounds for scroll clamping.
                if chart.chart_state.collapse_gaps {
                    chart.chart_state.data_time_start = 0.0;
                    chart.chart_state.data_time_end = len as f64;
                } else {
                    let first_ts = buffer.timestamps[0] as f64;
                    let last_ts = buffer.timestamps[len - 1] as f64;
                    chart.chart_state.data_time_start = first_ts;
                    chart.chart_state.data_time_end = last_ts;
                }

                // Only reset camera to default view when the user changed
                // symbol/timeframe. On config restore, the saved camera
                // position is already in place.
                if reset_camera {
                    let visible_count = 200.min(len);

                    if chart.chart_state.collapse_gaps {
                        let start_idx = (len - visible_count) as f64;
                        let end_idx =
                            len as f64 + (visible_count as f64 * 0.05);
                        chart.chart_state.camera.time_start = start_idx;
                        chart.chart_state.camera.time_end = end_idx;
                    } else {
                        let last_ts = buffer.timestamps[len - 1] as f64;
                        let first_visible_ts =
                            buffer.timestamps[len - visible_count] as f64;
                        chart.chart_state.camera.time_start = first_visible_ts;
                        chart.chart_state.camera.time_end =
                            last_ts + (last_ts - first_visible_ts) * 0.05;
                    }

                    let range = (len - visible_count)..len;
                    let (low, high) = buffer.price_range(range);
                    let padding = (high - low) as f64 * 0.05;
                    chart.chart_state.camera.price_low = low as f64 - padding;
                    chart.chart_state.camera.price_high =
                        high as f64 + padding;
                }

                chart.chart_state.dirty.mark_camera();
            }

            self.status_message =
                format!("{}: {} candles at {}", symbol, count, tf.display_name());
        }
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
                task
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
                    chart.chart_state.dirty.mark_camera();
                }

                if !symbol.is_empty() {
                    self.load_test_data_for_chart(chart_id, &symbol, tf, true);
                }

                self.mark_config_dirty();
                Task::none()
            }

            Message::DataLoaded(_chart_id, _result) => {
                // Data is now loaded synchronously via TestDataProvider.
                // This message is retained for future async data sources.
                Task::none()
            }

            Message::AddChart => {
                if let Some(focused) = self.workspace.focus {
                    if let Some((new_id, _new_pane)) = self
                        .workspace
                        .split(pane_grid::Axis::Vertical, focused)
                    {
                        self.charts.insert(new_id, Self::make_empty_panel());
                        self.status_message = format!("Added {new_id}");
                    }
                }
                Task::none()
            }

            Message::CloseChart(id) => {
                if let Some(pane) = self.workspace.find_pane(id) {
                    if let Some(closed_id) = self.workspace.close(pane) {
                        self.charts.remove(&closed_id);
                        self.status_message = format!("Closed {closed_id}");
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
                let new_ids = self.workspace.apply_preset(&preset);
                for id in &new_ids {
                    self.charts
                        .entry(*id)
                        .or_insert_with(Self::make_empty_panel);
                }
                let active_ids: std::collections::HashSet<ChartId> =
                    self.workspace.chart_ids().into_iter().collect();
                self.charts.retain(|id, _| active_ids.contains(id));
                self.mark_config_dirty();
                Task::none()
            }

            Message::PaneFocused(pane) => {
                self.workspace.set_focus(pane);
                Task::none()
            }

            Message::PaneResized(pane_grid::ResizeEvent { split, ratio }) => {
                self.workspace.panes.resize(split, ratio);
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Picked { pane }) => {
                self.workspace.set_focus(pane);
                Task::none()
            }

            Message::PaneDragged(pane_grid::DragEvent::Dropped {
                pane,
                target,
            }) => {
                self.workspace.panes.drop(pane, target);
                Task::none()
            }

            Message::PaneDragged(_) => Task::none(),

            Message::PaneSplit(axis, pane) => {
                if let Some((new_id, _new_pane)) =
                    self.workspace.split(axis, pane)
                {
                    self.charts.insert(new_id, Self::make_empty_panel());
                    self.status_message =
                        format!("Split pane, added {new_id}");
                }
                Task::none()
            }

            Message::PaneClose(pane) => {
                if let Some(closed_id) = self.workspace.close(pane) {
                    self.charts.remove(&closed_id);
                    self.status_message = format!("Closed {closed_id}");
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::ChartViewportChanged(
                chart_id, old_w, old_h, new_w, new_h,
            ) => {
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let cam = &mut chart.chart_state.camera;
                    if old_w > 0 && old_h > 0 {
                        let w_ratio = new_w as f64 / old_w as f64;
                        let h_ratio = new_h as f64 / old_h as f64;

                        // Horizontal: anchor right edge, expand/contract left.
                        let time_range = cam.time_end - cam.time_start;
                        cam.time_start = cam.time_end - time_range * w_ratio;

                        // Vertical: anchor center, expand/contract both edges.
                        let price_center =
                            (cam.price_high + cam.price_low) / 2.0;
                        let half_range =
                            (cam.price_high - cam.price_low) / 2.0 * h_ratio;
                        cam.price_high = price_center + half_range;
                        cam.price_low = price_center - half_range;
                    }
                    // Update canonical viewport so the snapshot matches
                    // actual bounds on the next frame.
                    cam.viewport_width = new_w;
                    cam.viewport_height = new_h;
                    // Clear crosshair during resize so it doesn't linger.
                    chart.chart_state.crosshair_pos = None;
                    chart.chart_state.dirty.mark_camera();
                    chart.chart_state.dirty.mark_crosshair();
                }
                Task::none()
            }

            Message::ChartPan(chart_id, dx, dy) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    chart.chart_state.apply_action(
                        &midas_chart::ChartAction::Pan { dx, dy },
                    );
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
                    chart.chart_state.crosshair_pos = pos;
                    chart.chart_state.dirty.mark_crosshair();
                }
                Task::none()
            }

            Message::ChartCreateLevel(chart_id, price) => {
                self.focus_chart(chart_id);
                if let Some(chart) = self.charts.get_mut(&chart_id) {
                    let level_id = chart.chart_state.alloc_level_id();
                    chart.chart_state.levels.push(
                        midas_chart::levels::HorizontalLevel {
                            id: level_id,
                            price,
                            color: [0.22, 0.55, 0.95, 0.8],
                            line_width: 1.0,
                        },
                    );
                    chart.chart_state.dirty.mark_levels();
                    self.mark_config_dirty();
                }
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
                                let start_idx = data
                                    .find_index_by_time(cam.time_start as i64)
                                    as f64;
                                let end_idx = data
                                    .find_index_by_time(cam.time_end as i64)
                                    as f64
                                    + 1.0;
                                cam.time_start = start_idx;
                                cam.time_end = end_idx;
                                chart.chart_state.data_time_start = 0.0;
                                chart.chart_state.data_time_end =
                                    len as f64;
                            } else {
                                // Switching OFF: convert camera from index-space
                                // back to time-space.
                                let si = (cam.time_start.round() as usize)
                                    .min(len.saturating_sub(1));
                                let ei = (cam.time_end.round() as usize)
                                    .min(len.saturating_sub(1));
                                cam.time_start =
                                    data.timestamps[si] as f64;
                                cam.time_end =
                                    data.timestamps[ei] as f64;
                                chart.chart_state.data_time_start =
                                    data.timestamps[0] as f64;
                                chart.chart_state.data_time_end =
                                    data.timestamps[len - 1] as f64;
                            }
                        }
                    }
                    chart.chart_state.dirty.mark_camera();
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
                if !symbol.is_empty() {
                    self.load_test_data_for_chart(chart_id, &symbol, tf, true);
                }
                self.mark_config_dirty();
                Task::none()
            }

            Message::KeyPressed(key) => {
                self.handle_key_press(key);
                Task::none()
            }

            Message::ConfigSaved(result) => {
                match result {
                    Ok(()) => {
                        tracing::debug!("Config saved successfully");
                    }
                    Err(ref e) => {
                        tracing::warn!("Config save failed: {e}");
                        self.status_message =
                            format!("Config save failed: {e}");
                    }
                }
                Task::none()
            }

            Message::WindowCloseRequested => self.flush_config(),

            Message::PopOut(pane) => {
                if let Some(pane_state) = self.workspace.panes.get(pane) {
                    let chart_id = pane_state.chart_id;
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
                        let (win_id, open_task) =
                            window::open(window::Settings {
                                size: iced::Size::new(800.0, 500.0),
                                ..window::Settings::default()
                            });
                        self.floating_charts
                            .insert(win_id, floating_chart);
                        self.status_message =
                            format!("Popped out {title} to new window");
                        return open_task.map(|_id| Message::Tick);
                    }
                }
                Task::none()
            }

            Message::WindowMoved(x, y) => {
                self.window_position = Some((x, y));
                self.mark_config_dirty();
                // Re-query monitor size (window may have moved to a different monitor).
                if let Some(id) = self.main_window {
                    return window::monitor_size(id)
                        .map(Message::MonitorSizeResult);
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
                    self.monitor_size =
                        Some((s.width as u32, s.height as u32));
                }
                Task::none()
            }

            Message::MainWindowOpened(id) => {
                tracing::info!("Main window opened: {id}");
                self.main_window = Some(id);
                // Query the monitor size for config persistence.
                window::monitor_size(id).map(Message::MonitorSizeResult)
            }

            Message::FloatingWindowClosed(id) => {
                if let Some(chart) = self.floating_charts.remove(&id) {
                    tracing::info!(
                        "Floating window closed for {}",
                        chart.symbol
                    );
                }
                // If the main window was closed, exit the application.
                if self.main_window == Some(id) {
                    return self.flush_config().chain(iced::exit());
                }
                Task::none()
            }

            Message::Tick => self.maybe_save_config(),
        }
    }

    /// Handle keyboard shortcut actions.
    fn handle_key_press(&mut self, key: iced::keyboard::Key) {
        use iced::keyboard::key::Named;
        use iced::keyboard::Key;
        match key {
            Key::Character(ref c) => match c.as_str() {
                "1" => self.set_active_timeframe(Timeframe::M1),
                "2" => self.set_active_timeframe(Timeframe::M5),
                "3" => self.set_active_timeframe(Timeframe::M15),
                "4" => self.set_active_timeframe(Timeframe::H1),
                "5" => self.set_active_timeframe(Timeframe::H4),
                "6" => self.set_active_timeframe(Timeframe::D1),
                "7" => self.set_active_timeframe(Timeframe::W1),
                _ => {}
            },
            Key::Named(Named::F11) => {
                self.show_frame_overlay = !self.show_frame_overlay;
            }
            _ => {}
        }
    }

    /// Set the timeframe on the active chart and regenerate data.
    fn set_active_timeframe(&mut self, tf: Timeframe) {
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

            if !symbol.is_empty() {
                self.load_test_data_for_chart(id, &symbol, tf, true);
            }
        }
    }
}

// View functions (view, view_toolbar, view_content, view_pane_*, view_status_bar)
// are in app/views.rs.
