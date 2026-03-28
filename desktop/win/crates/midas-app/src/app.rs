//! Application state, Message enum, update logic, and view tree.
//!
//! This module implements the iced Elm architecture for Hand of Midas:
//! - `MidasApp`: the top-level state struct
//! - `Message`: all events the app can process
//! - `MidasApp::update()`: pure state transitions + async tasks
//! - `MidasApp::view()`: builds the widget tree each frame

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use iced::widget::pane_grid::{self, PaneGrid};
use iced::widget::{button, column, container, row, text, text_input, Row, Space};
use iced::{window, Color, Element, Fill, Task};

use midas_chart::camera::Camera2D;
use midas_chart::state::ChartState;
use midas_core::config::{AppConfig, ChartConfig, LevelConfig};
use midas_core::{ChartId, Timeframe};
use midas_data::CandleBuffer;
use midas_feed::TestDataProvider;

use crate::layout::{LayoutPresetKind, WorkspaceLayout};
use crate::theme;

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

        // Open the main window via the daemon. The returned Task produces
        // the window::Id once the OS window is created.
        let (main_id, open_task) = window::open(window::Settings {
            size: iced::Size::new(
                config.window.width as f32,
                config.window.height as f32,
            ),
            ..window::Settings::default()
        });

        let open_task = open_task.map(Message::MainWindowOpened);

        if config.charts.is_empty() {
            let (workspace, first_id) = WorkspaceLayout::single();
            let mut charts = HashMap::new();
            charts.insert(first_id, Self::make_empty_panel());

            let app = Self {
                charts,
                workspace,
                status_message: "Ready".into(),
                show_frame_overlay: false,
                config_path,
                config_dirty: false,
                last_config_save: Instant::now(),
                current_time,
                main_window: Some(main_id),
                floating_charts: HashMap::new(),
                test_data: TestDataProvider::new(),
            };

            (app, open_task)
        } else {
            let (mut workspace, first_id) = WorkspaceLayout::single();
            let mut charts = HashMap::new();

            let first_cfg = &config.charts[0];
            let first_tf = Timeframe::from_suffix(&first_cfg.timeframe)
                .unwrap_or(Timeframe::D1);
            let mut first_panel = Self::make_empty_panel();
            first_panel.symbol = first_cfg.symbol.clone();
            first_panel.timeframe = first_tf;
            Self::restore_levels(&first_cfg.levels, &mut first_panel);
            Self::restore_camera(first_cfg, &mut first_panel);
            charts.insert(first_id, first_panel);

            let first_pane = workspace.focus.unwrap();
            for chart_cfg in config.charts.iter().skip(1) {
                let tf = Timeframe::from_suffix(&chart_cfg.timeframe)
                    .unwrap_or(Timeframe::D1);
                let mut panel = Self::make_empty_panel();
                panel.symbol = chart_cfg.symbol.clone();
                panel.timeframe = tf;
                Self::restore_levels(&chart_cfg.levels, &mut panel);
                Self::restore_camera(chart_cfg, &mut panel);

                if let Some((new_id, _)) =
                    workspace.split(pane_grid::Axis::Vertical, first_pane)
                {
                    charts.insert(new_id, panel);
                }
            }

            workspace.set_focus(first_pane);

            let chart_count = charts.len();
            let mut app = Self {
                charts,
                workspace,
                status_message: format!(
                    "Restored {chart_count} chart(s) from config"
                ),
                show_frame_overlay: false,
                config_path,
                config_dirty: false,
                last_config_save: Instant::now(),
                current_time,
                main_window: Some(main_id),
                floating_charts: HashMap::new(),
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
                app.load_test_data_for_chart(id, &symbol, tf);
            }

            (app, open_task)
        }
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

        self.load_test_data_for_chart(chart_id, &symbol, tf);
        Task::none()
    }

    /// Generate test data at the given timeframe and install it in the chart.
    ///
    /// Called from symbol submit, timeframe change, and config restore.
    fn load_test_data_for_chart(
        &mut self,
        chart_id: ChartId,
        symbol: &str,
        tf: Timeframe,
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
                let visible_count = 200.min(len);

                if chart.chart_state.collapse_gaps {
                    // Index-space: camera X axis = candle indices.
                    chart.chart_state.data_time_start = 0.0;
                    chart.chart_state.data_time_end = len as f64;
                    let start_idx = (len - visible_count) as f64;
                    let end_idx = len as f64 + (visible_count as f64 * 0.05);
                    chart.chart_state.camera.time_start = start_idx;
                    chart.chart_state.camera.time_end = end_idx;
                } else {
                    // Time-space: camera X axis = epoch milliseconds.
                    let first_ts = buffer.timestamps[0] as f64;
                    let last_ts = buffer.timestamps[len - 1] as f64;
                    chart.chart_state.data_time_start = first_ts;
                    chart.chart_state.data_time_end = last_ts;
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
                chart.chart_state.camera.price_high = high as f64 + padding;
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

// ── Config persistence ───────────────────────────────────────────────

impl MidasApp {
    /// Build an `AppConfig` from the current application state.
    fn build_config(&self) -> AppConfig {
        let chart_configs: Vec<ChartConfig> = self
            .workspace
            .chart_ids()
            .iter()
            .filter_map(|id| self.charts.get(id))
            .map(|panel| {
                let cam = &panel.chart_state.camera;
                let levels = panel
                    .chart_state
                    .levels
                    .iter()
                    .map(|l| LevelConfig {
                        price: l.price,
                        color: l.color,
                        line_width: l.line_width,
                    })
                    .collect();
                ChartConfig {
                    symbol: panel.symbol.clone(),
                    timeframe: panel.timeframe.display_name().to_string(),
                    levels,
                    camera_time_start: Some(cam.time_start),
                    camera_time_end: Some(cam.time_end),
                    camera_price_low: Some(cam.price_low),
                    camera_price_high: Some(cam.price_high),
                    collapse_gaps: panel.chart_state.collapse_gaps,
                }
            })
            .collect();

        // Read actual viewport dimensions from the first chart's camera
        // as a reasonable proxy for window size. Falls back to defaults.
        let (win_w, win_h) = self
            .workspace
            .chart_ids()
            .first()
            .and_then(|id| self.charts.get(id))
            .map(|panel| {
                (
                    panel.chart_state.camera.viewport_width,
                    panel.chart_state.camera.viewport_height,
                )
            })
            .unwrap_or((1280, 800));

        AppConfig {
            window: midas_core::config::WindowConfig {
                width: win_w,
                height: win_h,
                maximized: false,
            },
            theme: midas_core::config::ThemeConfig {
                mode: "dark".into(),
            },
            charts: chart_configs,
        }
    }

    /// Mark the configuration as dirty so it will be saved on the next tick.
    fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
    }

    /// Save the configuration if dirty and debounce interval has elapsed.
    fn maybe_save_config(&mut self) -> Task<Message> {
        if !self.config_dirty {
            return Task::none();
        }
        let elapsed = self.last_config_save.elapsed().as_secs_f64();
        if elapsed < CONFIG_SAVE_DEBOUNCE_SECS {
            return Task::none();
        }
        self.flush_config()
    }

    /// Unconditionally save the configuration right now.
    fn flush_config(&mut self) -> Task<Message> {
        self.config_dirty = false;
        self.last_config_save = Instant::now();
        let config = self.build_config();
        let path = self.config_path.clone();

        Task::perform(
            async move {
                let result =
                    tokio::task::spawn_blocking(move || config.save(&path))
                        .await;
                match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(e) => Err(format!("task join error: {e}")),
                }
            },
            Message::ConfigSaved,
        )
    }
}

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
                    self.load_test_data_for_chart(chart_id, &symbol, tf);
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
                self.focus_chart(chart_id);
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
                    self.load_test_data_for_chart(chart_id, &symbol, tf);
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

            Message::MainWindowOpened(id) => {
                tracing::info!("Main window opened: {id}");
                self.main_window = Some(id);
                Task::none()
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
                self.load_test_data_for_chart(id, &symbol, tf);
            }
        }
    }
}

// ── View ──────────────────────────────────────────────────────────────

impl MidasApp {
    /// Build the widget tree for a given window.
    ///
    /// The main window shows toolbar + pane_grid + status bar.
    /// Floating chart windows show only the chart with a minimal header.
    pub fn view(&self, window_id: window::Id) -> Element<'_, Message> {
        // Check if this is a floating chart window.
        if let Some(chart) = self.floating_charts.get(&window_id) {
            return self.view_floating_chart(chart);
        }

        // Main window (or fallback for unknown windows).
        let toolbar = self.view_toolbar();
        let content = self.view_content();
        let status_bar = self.view_status_bar();
        column![toolbar, content, status_bar].into()
    }

    /// Build the view for a floating (pop-out) chart window.
    fn view_floating_chart<'a>(
        &'a self,
        chart: &'a ChartPanel,
    ) -> Element<'a, Message> {
        // If data is loaded, render via GPU Shader widget.
        if let (LoadState::Loaded, Some(ref data)) =
            (&chart.load_state, &chart.data)
        {
            let snapshot = crate::chart_widget::ChartRenderSnapshot {
                symbol: chart.symbol.clone(),
                data: Some(Arc::clone(data)),
                camera: chart.chart_state.camera.clone(),
                dirty: chart.chart_state.dirty.clone(),
                crosshair_pos: chart.chart_state.crosshair_pos,
                levels: chart.chart_state.levels.clone(),
                viewport_width: chart.chart_state.camera.viewport_width,
                viewport_height: chart.chart_state.camera.viewport_height,
                collapse_gaps: chart.chart_state.collapse_gaps,
                volume_scale: chart.chart_state.volume_scale,
                data_time_start: chart.chart_state.data_time_start,
                data_time_end: chart.chart_state.data_time_end,
            };
            // Use ChartId(0) for floating windows -- they don't participate
            // in the pane_grid's chart map.
            let program = crate::chart_widget::ChartProgram {
                chart_id: ChartId::new(0),
                snapshot,
            };
            let shader = crate::chart_widget::chart_shader(program);

            // Header bar with symbol and timeframe.
            let header = container(
                row![
                    text(&chart.symbol)
                        .size(13)
                        .color(Color::WHITE),
                    text(chart.timeframe.display_name())
                        .size(11)
                        .color(theme::TEXT_SECONDARY),
                ]
                .spacing(8)
                .padding([4, 8])
                .align_y(iced::Alignment::Center),
            )
            .width(Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.06, 0.08, 0.12, 0.90,
                ))),
                ..Default::default()
            });

            return column![header, shader].into();
        }

        // No data placeholder for floating window.
        let status_text = match &chart.load_state {
            LoadState::Empty => "No data loaded".to_string(),
            LoadState::Loading => "Loading...".to_string(),
            LoadState::Loaded => "Loaded".to_string(),
            LoadState::Error(e) => format!("Error: {e}"),
        };
        container(
            text(status_text).size(14).color(theme::TEXT_SECONDARY),
        )
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(|_theme| container::Style {
            background: Some(theme::CHART_EMPTY_BG.into()),
            ..Default::default()
        })
        .into()
    }

    /// Build the toolbar row (layout controls only).
    ///
    /// Per-chart ticker inputs and timeframe buttons live in each pane's
    /// title bar, so the global toolbar only holds layout presets, split
    /// actions, and the add-chart button.
    fn view_toolbar(&self) -> Element<'_, Message> {
        let layout_buttons = row![
            button(text("1").size(12))
                .on_press(Message::LayoutPreset(LayoutPresetKind::Single))
                .padding([4, 8])
                .style(hover_text_button_style),
            button(text("1|1").size(12))
                .on_press(Message::LayoutPreset(LayoutPresetKind::SplitH))
                .padding([4, 8])
                .style(hover_text_button_style),
            button(text("1/1").size(12))
                .on_press(Message::LayoutPreset(LayoutPresetKind::SplitV))
                .padding([4, 8])
                .style(hover_text_button_style),
            button(text("2x2").size(12))
                .on_press(Message::LayoutPreset(LayoutPresetKind::Grid2x2))
                .padding([4, 8])
                .style(hover_text_button_style),
        ]
        .spacing(2);

        let split_buttons = row![
            button(text("Split H").size(11))
                .on_press_maybe(self.workspace.focus.map(|p| {
                    Message::PaneSplit(pane_grid::Axis::Horizontal, p)
                }))
                .padding([4, 6])
                .style(hover_text_button_style),
            button(text("Split V").size(11))
                .on_press_maybe(self.workspace.focus.map(|p| {
                    Message::PaneSplit(pane_grid::Axis::Vertical, p)
                }))
                .padding([4, 6])
                .style(hover_text_button_style),
        ]
        .spacing(2);

        let add_btn = button(text("+").size(14))
            .on_press(Message::AddChart)
            .padding([4, 10])
            .style(hover_text_button_style);

        let toolbar_row = row![
            layout_buttons,
            split_buttons,
            add_btn,
        ]
        .spacing(8)
        .padding(6)
        .align_y(iced::Alignment::Center);

        container(toolbar_row)
            .width(Fill)
            .style(|_theme| container::Style {
                background: Some(theme::TOOLBAR_BG.into()),
                ..Default::default()
            })
            .into()
    }

    /// Build the main content area using iced's pane_grid widget.
    ///
    /// Each pane has a thin, semi-transparent TitleBar that serves as the
    /// drag handle for docking. The TitleBar contains the ticker symbol,
    /// per-panel timeframe buttons, a pop-out button, and a close button.
    fn view_content(&self) -> Element<'_, Message> {
        let focused_pane = self.workspace.focus;
        let pane_count = self.workspace.pane_count();

        let pane_grid_widget = PaneGrid::new(
            &self.workspace.panes,
            |pane, pane_state, _is_maximized| {
                let is_focused = focused_pane == Some(pane);
                let chart_id = pane_state.chart_id;

                // Build the drag-handle TitleBar.
                let title_bar = self.view_pane_title_bar(
                    chart_id,
                    pane,
                    is_focused,
                    pane_count,
                );

                let body = self.view_pane_body(chart_id, pane, is_focused);
                pane_grid::Content::new(body).title_bar(title_bar)
            },
        )
        .on_resize(6, Message::PaneResized)
        .on_drag(Message::PaneDragged)
        .style(|_theme| pane_grid::Style {
            hovered_region: pane_grid::Highlight {
                background: iced::Background::Color(Color::from_rgba(
                    0.2, 0.4, 0.8, 0.25,
                )),
                border: iced::Border {
                    color: Color::from_rgba(0.3, 0.5, 1.0, 0.6),
                    width: 2.0,
                    radius: 0.0.into(),
                },
            },
            hovered_split: pane_grid::Line {
                color: Color::from_rgba(0.3, 0.5, 1.0, 0.8),
                width: 2.0,
            },
            picked_split: pane_grid::Line {
                color: Color::from_rgba(0.3, 0.5, 1.0, 1.0),
                width: 3.0,
            },
        })
        .width(Fill)
        .height(Fill)
        .spacing(1);

        container(pane_grid_widget)
            .width(Fill)
            .height(Fill)
            .style(|_theme| container::Style {
                background: Some(theme::BACKGROUND.into()),
                ..Default::default()
            })
            .into()
    }

    /// Build the semi-transparent TitleBar for a pane.
    ///
    /// This serves as the drag handle for pane_grid's built-in docking.
    /// The title bar content (empty space) is the draggable area.
    /// Controls (ticker input + tf buttons + actions) are excluded from drag.
    /// Layout: `[drag area] | [TICKER_INPUT] [1m|5m|15m|1H|4H|D|W] [⧉] [×]`
    fn view_pane_title_bar(
        &self,
        chart_id: ChartId,
        pane: pane_grid::Pane,
        is_focused: bool,
        pane_count: usize,
    ) -> pane_grid::TitleBar<'_, Message> {
        let _chart = self.charts.get(&chart_id);

        // The title bar "content" is an empty spacer — the entire area
        // is the drag handle. All interactive widgets go in controls().
        let title_content = Space::new().width(Fill).height(24);

        // Build controls: ticker input + timeframe buttons + pop-out + close.
        let controls_row = self.view_title_bar_controls(
            chart_id,
            pane,
            is_focused,
            pane_count,
        );

        pane_grid::TitleBar::new(title_content)
            .controls(controls_row)
            .padding([2, 4])
            .always_show_controls()
            .style(move |_theme| container::Style {
                background: Some(iced::Background::Color(
                    Color::from_rgba(0.06, 0.08, 0.12, 0.85),
                )),
                border: iced::Border {
                    color: if is_focused {
                        theme::CHART_ACTIVE_BORDER
                    } else {
                        Color::TRANSPARENT
                    },
                    width: if is_focused { 1.0 } else { 0.0 },
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
    }

    /// Build the controls area of a pane's TitleBar.
    ///
    /// This area is excluded from the drag handle, so inputs and buttons
    /// remain interactive even while drag-and-drop is enabled.
    /// Layout: `[TICKER_INPUT] [1m|5m|15m|1H|4H|D|W] [fill] [⧉] [×]`
    fn view_title_bar_controls(
        &self,
        chart_id: ChartId,
        pane: pane_grid::Pane,
        _is_focused: bool,
        pane_count: usize,
    ) -> Element<'_, Message> {
        let chart = self.charts.get(&chart_id);
        let panel_tf = chart.map(|c| c.timeframe).unwrap_or(Timeframe::D1);
        let symbol_input_value = chart
            .map(|c| c.symbol_input.as_str())
            .unwrap_or("");

        // Per-chart ticker input — compact, inline.
        let ticker_input = text_input("SYMBOL", symbol_input_value)
            .on_input(move |val| Message::PanelSymbolInputChanged(chart_id, val))
            .on_submit(Message::PanelSymbolSubmitted(chart_id))
            .width(70)
            .size(11)
            .padding([2, 4]);

        // Timeframe buttons.
        let timeframes = [
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
        ];
        let tf_buttons: Vec<Element<'_, Message>> = timeframes
            .iter()
            .map(|&tf| {
                let label = tf.display_name();
                let is_active = panel_tf == tf;
                if is_active {
                    button(text(label).size(10).color(Color::WHITE))
                        .on_press(Message::PanelTimeframeSelected(chart_id, tf))
                        .padding([1, 4])
                        .style(button::primary)
                        .into()
                } else {
                    button(text(label).size(10))
                        .on_press(Message::PanelTimeframeSelected(chart_id, tf))
                        .padding([1, 4])
                        .style(button::text)
                        .into()
                }
            })
            .collect();
        let tf_row = Row::with_children(tf_buttons).spacing(1);

        // Collapse-gaps toggle button (shown as "G" icon).
        let collapse_active = chart
            .map(|c| c.chart_state.collapse_gaps)
            .unwrap_or(false);
        let collapse_btn = if collapse_active {
            button(text("G").size(10).color(Color::WHITE))
                .on_press(Message::ToggleCollapseGaps(chart_id))
                .padding([1, 4])
                .style(button::primary)
        } else {
            button(text("G").size(10))
                .on_press(Message::ToggleCollapseGaps(chart_id))
                .padding([1, 4])
                .style(button::text)
        };

        // Pop-out button.
        let pop_out_btn = button(text("\u{29C9}").size(12))
            .on_press(Message::PopOut(pane))
            .padding([1, 5])
            .style(button::text);

        // Close button (only if more than one pane).
        let close_btn: Element<'_, Message> = if pane_count > 1 {
            button(text("\u{00D7}").size(12))
                .on_press(Message::PaneClose(pane))
                .padding([1, 5])
                .style(button::text)
                .into()
        } else {
            Space::new().width(0).height(0).into()
        };

        // Reset button — fits view to all data.
        let reset_btn = button(text("R").size(10))
            .on_press(Message::ResetChart(chart_id))
            .padding([1, 4])
            .style(button::text);

        row![ticker_input, tf_row, collapse_btn, reset_btn, Space::new().width(Fill), pop_out_btn, close_btn]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into()
    }

    /// Render the body content of a single pane (chart panel).
    ///
    /// Controls are now in the pane_grid TitleBar, so the body is just
    /// the chart shader or a placeholder.
    fn view_pane_body(
        &self,
        chart_id: ChartId,
        _pane: pane_grid::Pane,
        is_focused: bool,
    ) -> Element<'_, Message> {
        let chart = match self.charts.get(&chart_id) {
            Some(c) => c,
            None => return self.view_empty_placeholder(),
        };
        let border_color = if is_focused {
            theme::CHART_ACTIVE_BORDER
        } else {
            theme::CHART_INACTIVE_BORDER
        };

        // If data is loaded, render via GPU Shader widget.
        if let (LoadState::Loaded, Some(ref data)) =
            (&chart.load_state, &chart.data)
        {
            let snapshot = crate::chart_widget::ChartRenderSnapshot {
                symbol: chart.symbol.clone(),
                data: Some(Arc::clone(data)),
                camera: chart.chart_state.camera.clone(),
                dirty: chart.chart_state.dirty.clone(),
                crosshair_pos: chart.chart_state.crosshair_pos,
                levels: chart.chart_state.levels.clone(),
                viewport_width: chart.chart_state.camera.viewport_width,
                viewport_height: chart.chart_state.camera.viewport_height,
                collapse_gaps: chart.chart_state.collapse_gaps,
                volume_scale: chart.chart_state.volume_scale,
                data_time_start: chart.chart_state.data_time_start,
                data_time_end: chart.chart_state.data_time_end,
            };
            let program = crate::chart_widget::ChartProgram {
                chart_id,
                snapshot,
            };
            let shader = crate::chart_widget::chart_shader(program);

            return container(shader)
                .width(Fill)
                .height(Fill)
                .style(move |_theme| container::Style {
                    border: iced::Border {
                        color: border_color,
                        width: if is_focused { 2.0 } else { 1.0 },
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                })
                .into();
        }

        // Placeholder for empty/loading/error states.
        let status_text = match &chart.load_state {
            LoadState::Empty => {
                "No data -- type a symbol and press Enter".to_string()
            }
            LoadState::Loading => "Loading...".to_string(),
            LoadState::Loaded => "Loaded".to_string(),
            LoadState::Error(e) => format!("Error: {e}"),
        };
        let bg_color = theme::CHART_EMPTY_BG;

        container(
            text(status_text).size(14).color(theme::TEXT_SECONDARY),
        )
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .style(move |_theme| container::Style {
            background: Some(bg_color.into()),
            border: iced::Border {
                color: border_color,
                width: if is_focused { 2.0 } else { 1.0 },
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
    }

    /// Render an empty placeholder when no chart data exists.
    fn view_empty_placeholder(&self) -> Element<'_, Message> {
        container(
            text("Empty")
                .size(16)
                .color(theme::TEXT_MUTED)
                .align_x(iced::alignment::Horizontal::Center),
        )
        .width(Fill)
        .height(Fill)
        .center(Fill)
        .style(|_theme| container::Style {
            background: Some(theme::CHART_EMPTY_BG.into()),
            border: iced::Border {
                color: theme::CHART_INACTIVE_BORDER,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
    }

    /// Build the status bar at the bottom of the window.
    fn view_status_bar(&self) -> Element<'_, Message> {
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
        let pane_count = self.workspace.pane_count();
        let overlay_indicator = if self.show_frame_overlay {
            " | F11: overlay ON"
        } else {
            ""
        };
        let status_row = row![
            text(&self.status_message)
                .size(12)
                .color(theme::TEXT_SECONDARY),
            Space::new().width(Fill),
            text(format!(
                "{active_info} | {pane_count} pane(s){overlay_indicator} | {}",
                self.current_time
            ))
            .size(12)
            .color(theme::TEXT_MUTED),
        ]
        .padding([4, 8])
        .align_y(iced::Alignment::Center);

        container(status_row)
            .width(Fill)
            .style(|_theme| container::Style {
                background: Some(theme::STATUS_BAR_BG.into()),
                ..Default::default()
            })
            .into()
    }
}

// ── Button style helpers ──────────────────────────────────────────────

/// Button style with hover highlight: muted text by default, white text
/// and subtle background on hover/press.
fn hover_text_button_style(
    _theme: &iced::Theme,
    status: button::Status,
) -> button::Style {
    let text_color = match status {
        button::Status::Hovered | button::Status::Pressed => Color::WHITE,
        _ => theme::TEXT_MUTED,
    };
    let background = match status {
        button::Status::Hovered => {
            Some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.1)))
        }
        button::Status::Pressed => {
            Some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.15)))
        }
        _ => None,
    };
    button::Style {
        text_color,
        background,
        ..Default::default()
    }
}
