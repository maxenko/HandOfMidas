# 05 - App Integration: MidasApp + ProviderRegistry

## Overview

This document specifies every change to `MidasApp` required to replace the
hardcoded `TestDataProvider` and standalone `DbHandle` with the unified
`ProviderRegistry`. It covers struct field changes, new `Message` variants,
rewritten data loading handlers, the startup sequence, provider switching,
and backward compatibility guarantees.

**Prerequisite reads:**
- [04-provider-registry.md](04-provider-registry.md) -- ProviderRegistry API
- `desktop/win/crates/midas-app/src/app.rs` -- current MidasApp implementation

---

## 1. MidasApp Field Changes

### Fields Removed

```rust
// REMOVE these two fields from MidasApp:

/// Deterministic test data generator. Any ticker produces instant data.
test_data: TestDataProvider,

/// DuckDB persistent cache handle. None if disabled or failed to open.
pub store: Option<midas_store::DbHandle>,
```

**Why:** `TestDataProvider` is now wrapped inside `TestProvider` (which
implements `DataProvider`). `DbHandle` is now owned by `CachingProvider`
(which also implements `DataProvider`). Both live inside the registry.

### Field Added

```rust
// ADD this field to MidasApp:

/// Central registry of all data providers and order brokers.
/// Replaces the separate `test_data` and `store` fields.
pub providers: ProviderRegistry,
```

### Updated MidasApp Struct

```rust
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
    /// The window ID of the main application window.
    pub main_window: Option<window::Id>,
    /// Floating chart windows popped out from the main pane grid.
    pub floating_charts: HashMap<window::Id, ChartPanel>,
    /// Last known main window position.
    pub window_position: Option<(i32, i32)>,
    /// Last known main window size.
    pub window_size: (u32, u32),
    /// Size of the monitor the main window is on.
    pub monitor_size: Option<(u32, u32)>,
    /// Centralized per-ticker level store, shared across all charts.
    pub level_store: LevelStore,
    /// Whether level placement mode is globally active.
    pub level_placing: bool,
    /// Active placement preview state.
    pub placing_preview: Option<(ChartId, String, f64)>,
    /// Cross-chart crosshair sync state.
    pub crosshair_sync: Option<(ChartId, i64, f64, String)>,
    /// Central registry of all data providers and order brokers.
    pub providers: ProviderRegistry,                           // NEW
    /// All watchlist panels keyed by stable WatchlistId.
    pub watchlists: HashMap<WatchlistId, WatchlistPanel>,
}
```

### Import Changes in app.rs

```rust
// REMOVE:
use midas_feed::TestDataProvider;

// ADD:
use crate::registry::ProviderRegistry;
use midas_core::provider::{DataProvider, ProviderError};
```

---

## 2. New Message Variants

Add three new variants to the `Message` enum:

```rust
pub enum Message {
    // ... existing variants ...

    // -- Provider management --
    /// User selected a data provider from the toolbar dropdown.
    /// The `String` is the provider's display name.
    DataProviderSelected(String),

    /// Trigger a reload of all charts that currently have data loaded.
    /// Emitted internally after a provider switch completes.
    AllChartsReloadRequested,

    /// A provider's connection status changed (name of provider).
    /// Used for status bar updates and future health monitoring.
    ProviderStatusChanged(String),
}
```

### Message Flow Diagram

```
User clicks dropdown
    |
    v
DataProviderSelected(name)
    |
    v
update(): resolve name → index, set_active_data(idx) + build reload tasks
    |
    +---> Task::perform(provider.get_candles(...))  x N charts
    |         |
    |         v
    |     DataLoaded(chart_id, result)  -- existing variant, reused
    |
    +---> status_message = "Switched to ..."
    |
    +---> mark_config_dirty()
```

### Why Not Reuse AllChartsReloadRequested

The `DataProviderSelected` handler builds the reload tasks directly rather
than emitting `AllChartsReloadRequested` as a separate message. This avoids
an extra update cycle (frame) of latency. `AllChartsReloadRequested` is kept
as a separate variant for other callers that need to trigger a full reload
(e.g., reconnection after network drop, config hot-reload).

---

## 3. Rewritten Data Loading Flow

### Helper: days_for_timeframe

Extract the existing timeframe-to-days mapping into a shared helper:

```rust
/// Determine how many calendar days of data to request based on timeframe.
///
/// Coarser timeframes request more history so charts are not empty.
/// Used by all data loading paths (symbol submit, timeframe change,
/// provider switch, startup restore).
fn days_for_timeframe(tf: Timeframe) -> u32 {
    match tf.as_secs() {
        s if s >= Timeframe::W1.as_secs() => 3650, // ~10 years
        s if s >= Timeframe::D1.as_secs() => 730,  // ~2 years
        s if s >= Timeframe::H1.as_secs() => 90,   // ~3 months
        s if s >= Timeframe::M15.as_secs() => 30,  // ~1 month
        _ => 10,                                    // <=M5: ~10 days
    }
}
```

### Helper: apply_candle_data

Extract the chart-data-application logic into a helper that both the sync
path (startup fast-path) and the async `DataLoaded` handler share:

```rust
/// Apply loaded candle data to a chart panel.
///
/// Sets `chart.data`, updates `load_state`, configures data bounds for
/// scroll clamping, and optionally resets the camera to show the last
/// 200 candles.
///
/// This is the single source of truth for "data arrived, update the chart."
/// Called from both `DataLoaded` handler and startup restore.
fn apply_candle_data(
    chart: &mut ChartPanel,
    buffer: Arc<CandleBuffer>,
    reset_camera: bool,
) {
    chart.data = Some(Arc::clone(&buffer));
    chart.load_state = LoadState::Loaded;
    chart.chart_state.dirty.mark_data();

    if buffer.is_empty() {
        return;
    }

    let len = buffer.len();

    // Set data bounds for scroll clamping.
    if chart.chart_state.collapse_gaps {
        chart.chart_state.data_time_start = 0.0;
        chart.chart_state.data_time_end = len as f64;
    } else {
        let first_ts = buffer.timestamps[0] as f64;
        let last_ts = buffer.timestamps[len - 1] as f64;
        chart.chart_state.data_time_start = first_ts;
        chart.chart_state.data_time_end = last_ts;
    }

    // Only reset camera when the user changed symbol/timeframe.
    // On config restore, the saved camera position is already in place.
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
            chart.chart_state.camera.time_end =
                last_ts + (last_ts - first_visible_ts) * 0.05;
        }

        let range = (len - visible_count)..len;
        let (low, high) = buffer.price_range(range);
        let padding = (high - low) as f64 * 0.05;
        chart.chart_state.camera.price_low = low as f64 - padding;
        chart.chart_state.camera.price_high = high as f64 + padding;
    }

    chart.chart_state.dirty.mark_camera();
}
```

### Helper: load_chart_async

Build a `Task` that loads data for a single chart from the active provider:

```rust
impl MidasApp {
    /// Create an async Task that loads candle data for a chart.
    ///
    /// Clones the active provider (Arc) into a closure, spawns the async
    /// `get_candles()` call, and maps the result to `DataLoaded`.
    fn load_chart_async(&self, chart_id: ChartId, symbol: &str, tf: Timeframe) -> Task<Message> {
        let provider = self.providers.active_data().clone();
        let symbol = symbol.to_uppercase();
        let days = days_for_timeframe(tf);

        Task::perform(
            async move {
                provider.get_candles(&symbol, tf, days).await
            },
            move |result| {
                Message::DataLoaded(
                    chart_id,
                    result
                        .map(|buf| Arc::new(buf))
                        .map_err(|e| e.to_string()),
                )
            },
        )
    }
}
```

---

## 4. Rewritten Message Handlers

### PanelSymbolSubmitted

**Before (synchronous, TestDataProvider-specific):**
```rust
Message::PanelSymbolSubmitted(chart_id) => {
    // ... extract symbol ...
    let task = self.load_symbol_for_chart(chart_id, &symbol);
    self.mark_config_dirty();
    task
}

fn load_symbol_for_chart(&mut self, chart_id: ChartId, symbol: &str) -> Task<Message> {
    // ... validate symbol ...
    self.load_test_data_for_chart(chart_id, &symbol, tf, true);
    Task::none()
}
```

**After (async, provider-agnostic):**
```rust
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

    // Update chart state immediately.
    let tf = if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.symbol = symbol.clone();
        chart.symbol_input = symbol.clone();
        chart.load_state = LoadState::Loading;
        chart.chart_state.dirty.mark_data();
        chart.timeframe
    } else {
        return Task::none();
    };

    self.status_message = format!("Loading {}...", symbol);
    self.mark_config_dirty();

    // Dispatch async load via the active provider.
    self.load_chart_async(chart_id, &symbol, tf)
}
```

### PanelTimeframeSelected

**Before:**
```rust
Message::PanelTimeframeSelected(chart_id, tf) => {
    // ... get symbol ...
    if !symbol.is_empty() {
        self.load_test_data_for_chart(chart_id, &symbol, tf, true);
    }
    self.mark_config_dirty();
    Task::none()
}
```

**After:**
```rust
Message::PanelTimeframeSelected(chart_id, tf) => {
    self.focus_chart(chart_id);
    let symbol = self
        .charts
        .get(&chart_id)
        .map(|c| c.symbol.clone())
        .unwrap_or_default();

    if let Some(chart) = self.charts.get_mut(&chart_id) {
        chart.timeframe = tf;
        chart.chart_state.dirty.mark_camera();
    }

    let task = if !symbol.is_empty() {
        // Set loading state.
        if let Some(chart) = self.charts.get_mut(&chart_id) {
            chart.load_state = LoadState::Loading;
        }
        self.status_message = format!("Loading {} at {}...", symbol, tf.display_name());
        self.load_chart_async(chart_id, &symbol, tf)
    } else {
        Task::none()
    };

    self.mark_config_dirty();
    task
}
```

### DataLoaded (rewritten from no-op to functional)

**Before:**
```rust
Message::DataLoaded(_chart_id, _result) => {
    // Data is now loaded synchronously via TestDataProvider.
    // This message is retained for future async data sources.
    Task::none()
}
```

**After:**
```rust
Message::DataLoaded(chart_id, result) => {
    match result {
        Ok(buffer) => {
            let count = buffer.len();
            let symbol = self
                .charts
                .get(&chart_id)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();
            let tf = self
                .charts
                .get(&chart_id)
                .map(|c| c.timeframe)
                .unwrap_or(Timeframe::D1);

            if let Some(chart) = self.charts.get_mut(&chart_id) {
                Self::apply_candle_data(chart, buffer, true);
            }

            self.status_message =
                format!("{}: {} candles at {}", symbol, count, tf.display_name());
        }
        Err(error) => {
            tracing::warn!(chart_id = ?chart_id, error = %error, "data load failed");
            if let Some(chart) = self.charts.get_mut(&chart_id) {
                chart.load_state = LoadState::Error(error.clone());
            }
            self.status_message = format!("Load failed: {}", error);
        }
    }
    Task::none()
}
```

### DataProviderSelected (new)

```rust
Message::DataProviderSelected(name) => {
    if let Some(idx) = self.providers.find_data_provider_by_name(&name) {
        if !self.providers.set_active_data(idx) {
            // Same provider already active.
            return Task::none();
        }
    } else {
        // Unknown provider name.
        return Task::none();
    }

    let provider_name = name;
    tracing::info!(provider = %provider_name, "switched data provider");

    // Build reload tasks for all charts that currently have data.
    let mut tasks: Vec<Task<Message>> = Vec::new();

    let charts_to_reload: Vec<(ChartId, String, Timeframe)> = self
        .charts
        .iter()
        .filter(|(_, panel)| !panel.symbol.is_empty() && panel.data.is_some())
        .map(|(&id, panel)| (id, panel.symbol.clone(), panel.timeframe))
        .collect();

    for (chart_id, symbol, tf) in &charts_to_reload {
        // Set loading state on each chart.
        if let Some(chart) = self.charts.get_mut(chart_id) {
            chart.load_state = LoadState::Loading;
            chart.chart_state.dirty.mark_data();
        }
        tasks.push(self.load_chart_async(*chart_id, symbol, *tf));
    }

    self.status_message = format!(
        "Switched to {} (reloading {} chart{})",
        provider_name,
        charts_to_reload.len(),
        if charts_to_reload.len() == 1 { "" } else { "s" }
    );

    self.mark_config_dirty();

    if tasks.is_empty() {
        Task::none()
    } else {
        Task::batch(tasks)
    }
}
```

### AllChartsReloadRequested (new)

```rust
Message::AllChartsReloadRequested => {
    let mut tasks: Vec<Task<Message>> = Vec::new();

    let charts_to_reload: Vec<(ChartId, String, Timeframe)> = self
        .charts
        .iter()
        .filter(|(_, panel)| !panel.symbol.is_empty())
        .map(|(&id, panel)| (id, panel.symbol.clone(), panel.timeframe))
        .collect();

    for (chart_id, symbol, tf) in &charts_to_reload {
        if let Some(chart) = self.charts.get_mut(chart_id) {
            chart.load_state = LoadState::Loading;
            chart.chart_state.dirty.mark_data();
        }
        tasks.push(self.load_chart_async(*chart_id, symbol, *tf));
    }

    if charts_to_reload.is_empty() {
        self.status_message = "No charts to reload".into();
    } else {
        self.status_message = format!("Reloading {} chart(s)...", charts_to_reload.len());
    }

    if tasks.is_empty() {
        Task::none()
    } else {
        Task::batch(tasks)
    }
}
```

### ProviderStatusChanged (new)

```rust
Message::ProviderStatusChanged(provider_name) => {
    let is_connected = self
        .providers
        .data_provider_names()
        .iter()
        .zip(0..)
        .find(|(name, _)| **name == provider_name)
        .and_then(|(_, idx)| self.providers.data_provider(idx))
        .map(|p| p.is_connected())
        .unwrap_or(false);

    if is_connected {
        self.status_message = format!("{} connected", provider_name);
    } else {
        self.status_message = format!("{} disconnected", provider_name);
    }
    Task::none()
}
```

---

## 5. Other Handlers That Call load_test_data_for_chart

The following existing handlers currently call `self.load_test_data_for_chart()`
and must be updated to use `self.load_chart_async()`:

### ToggleCollapseGaps

```rust
// Current (around line 1357 in app.rs):
self.load_test_data_for_chart(chart_id, &symbol, tf, true);

// Replace with:
if let Some(chart) = self.charts.get_mut(&chart_id) {
    chart.load_state = LoadState::Loading;
}
return self.load_chart_async(chart_id, &symbol, tf);
```

### Watchlist symbol click / chart restore from floating

Any place that calls `load_test_data_for_chart` must be replaced with the
async pattern. Search the codebase for all call sites:

```
grep -n "load_test_data_for_chart" desktop/win/crates/midas-app/src/app.rs
```

Each call site follows the same transformation pattern:

**Before:**
```rust
self.load_test_data_for_chart(chart_id, &symbol, tf, reset_camera);
Task::none()
```

**After:**
```rust
if let Some(chart) = self.charts.get_mut(&chart_id) {
    chart.load_state = LoadState::Loading;
}
self.load_chart_async(chart_id, &symbol, tf)
```

Note: The `reset_camera` parameter is handled differently now. In the async
flow, `DataLoaded` always calls `apply_candle_data(chart, buffer, true)` --
camera is always reset when data is explicitly loaded. For config restore
(where `reset_camera = false`), a different code path is used (see Section 6).

### Complete Call Site Inventory

| Location | Current Call | New Pattern |
|----------|-------------|-------------|
| `MidasApp::new()` startup restore | `app.load_test_data_for_chart(id, &symbol, tf, false)` | Startup fast-path (see Section 6) |
| `PanelSymbolSubmitted` | `self.load_symbol_for_chart(...)` which calls `load_test_data_for_chart` | `self.load_chart_async(...)` |
| `PanelTimeframeSelected` | `self.load_test_data_for_chart(...)` | `self.load_chart_async(...)` |
| `ToggleCollapseGaps` | `self.load_test_data_for_chart(...)` | `self.load_chart_async(...)` |
| Floating window restore | `self.load_test_data_for_chart(...)` | `self.load_chart_async(...)` |

### Removing load_test_data_for_chart and load_symbol_for_chart

After all call sites are migrated, **delete** both methods:

```rust
// DELETE:
fn load_test_data_for_chart(&mut self, chart_id: ChartId, symbol: &str, tf: Timeframe, reset_camera: bool) { ... }
fn load_symbol_for_chart(&mut self, chart_id: ChartId, symbol: &str) -> Task<Message> { ... }
```

They are fully replaced by `load_chart_async()` + `apply_candle_data()`.

---

## 6. Startup Sequence

### Current Startup (Synchronous)

```rust
// In MidasApp::new():
let mut app = Self {
    // ...
    test_data: TestDataProvider::new(),
    store: if config.store.enabled { Some(DbHandle::open(...)) } else { None },
    // ...
};

// Auto-load test data synchronously.
for (id, symbol, tf) in chart_ids {
    app.load_test_data_for_chart(id, &symbol, tf, false);
}

(app, open_task)
```

### New Startup (Provider-Aware)

```rust
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
    let initial_position = Self::validate_saved_position(&config.window);
    let initial_size = (config.window.width, config.window.height);

    let (main_id, open_task) = window::open(window::Settings {
        size: iced::Size::new(config.window.width as f32, config.window.height as f32),
        position: initial_position,
        ..window::Settings::default()
    });
    let open_task = open_task.map(Message::MainWindowOpened);

    // Build workspace and charts from config.
    let (workspace, charts, status_message) = if config.charts.is_empty() {
        let (ws, first_id) = WorkspaceLayout::single();
        let mut charts = HashMap::new();
        charts.insert(first_id, Self::make_empty_panel());
        (ws, charts, "Ready".to_string())
    } else {
        // ... existing chart restore logic (unchanged) ...
        let (mut ws, first_id) = WorkspaceLayout::single();
        let mut charts = HashMap::new();
        let first_cfg = &config.charts[0];
        let panel = Self::restore_panel(first_cfg);
        charts.insert(first_id, panel);
        let first_pane = ws.focus.unwrap();
        for chart_cfg in config.charts.iter().skip(1) {
            let panel = Self::restore_panel(chart_cfg);
            if let Some((new_id, _)) = ws.split(pane_grid::Axis::Vertical, first_pane) {
                charts.insert(new_id, panel);
            }
        }
        ws.set_focus(first_pane);
        let n = charts.len();
        (ws, charts, format!("Restored {n} chart(s) from config"))
    };

    let level_store = LevelStore::from_config(&config.levels);

    // Restore watchlists.
    let mut watchlists = HashMap::new();
    for wl_cfg in &config.watchlists {
        let id = workspace.next_watchlist_id();
        watchlists.insert(id, WatchlistPanel::from_config(id, wl_cfg));
    }

    // ── Provider setup ───────────────────────────────────────────────
    let providers = Self::build_provider_registry(&config, &config_path);

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
        providers,
        watchlists,
    };

    // ── Startup data loading ─────────────────────────────────────────
    //
    // Build async tasks to load data for all restored charts.
    // Unlike the old synchronous path, this uses the active provider
    // which may involve DuckDB cache lookups or network calls.
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

    // Combine the window-open task with all data loading tasks.
    let startup_task = if load_tasks.is_empty() {
        open_task
    } else {
        load_tasks.push(open_task);
        Task::batch(load_tasks)
    };

    (app, startup_task)
}
```

### Startup Data Restore: reset_camera = false

During startup, charts restored from config should NOT reset the camera
(the saved camera position is already set by `restore_panel()`). But
`DataLoaded` always passes `reset_camera = true`.

To handle this, use a separate message variant for startup loads:

```rust
/// Data loaded during startup restore (does not reset camera).
DataRestoredFromStartup(ChartId, Result<Arc<CandleBuffer>, String>),
```

And a corresponding helper + handler:

```rust
impl MidasApp {
    /// Like `load_chart_async`, but maps to `DataRestoredFromStartup`
    /// instead of `DataLoaded` (preserves saved camera position).
    fn load_chart_async_restore(
        &self,
        chart_id: ChartId,
        symbol: &str,
        tf: Timeframe,
    ) -> Task<Message> {
        let provider = self.providers.active_data().clone();
        let symbol = symbol.to_uppercase();
        let days = days_for_timeframe(tf);

        Task::perform(
            async move {
                provider.get_candles(&symbol, tf, days).await
            },
            move |result| {
                Message::DataRestoredFromStartup(
                    chart_id,
                    result
                        .map(|buf| Arc::new(buf))
                        .map_err(|e| e.to_string()),
                )
            },
        )
    }
}

// In update():
Message::DataRestoredFromStartup(chart_id, result) => {
    match result {
        Ok(buffer) => {
            let count = buffer.len();
            let symbol = self
                .charts
                .get(&chart_id)
                .map(|c| c.symbol.clone())
                .unwrap_or_default();

            if let Some(chart) = self.charts.get_mut(&chart_id) {
                // reset_camera = false: preserve saved camera position.
                Self::apply_candle_data(chart, buffer, false);
            }

            tracing::debug!(
                chart_id = ?chart_id,
                symbol = %symbol,
                count = count,
                "startup restore complete"
            );
        }
        Err(error) => {
            tracing::warn!(chart_id = ?chart_id, error = %error, "startup restore failed");
            if let Some(chart) = self.charts.get_mut(&chart_id) {
                chart.load_state = LoadState::Error(error);
            }
        }
    }
    Task::none()
}
```

### build_provider_registry Helper

```rust
impl MidasApp {
    /// Build the provider registry from config.
    ///
    /// Creates providers in a fixed order, wraps in CachingProvider if
    /// DuckDB is enabled, and restores the active selection from config.
    fn build_provider_registry(config: &AppConfig, config_path: &Path) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();

        // 1. Create the base test provider (always available).
        let test_provider: Arc<dyn DataProvider> = Arc::new(
            midas_feed::TestProvider::new()
        );

        // 2. Wrap in CachingProvider if DuckDB store is enabled.
        let primary_provider: Arc<dyn DataProvider> = if config.store.enabled {
            let data_dir = config_path
                .parent()
                .unwrap_or_else(|| Path::new("."));
            let db_path = data_dir.join(&config.store.path);

            match Self::try_open_store(&db_path, config) {
                Some(db_handle) => {
                    tracing::info!(
                        path = %db_path.display(),
                        "DuckDB store enabled, using CachingProvider"
                    );
                    Arc::new(midas_store::CachingProvider::new(
                        test_provider.clone(),
                        db_handle,
                    ))
                }
                None => {
                    tracing::warn!(
                        "DuckDB failed to open, falling back to raw TestProvider"
                    );
                    test_provider.clone()
                }
            }
        } else {
            tracing::info!("DuckDB store disabled, using raw TestProvider");
            test_provider.clone()
        };

        registry.register_data_provider(primary_provider);

        // Future: register additional providers here.
        // if let Some(ib_cfg) = &config.ib {
        //     let ib = Arc::new(IbDataProvider::new(ib_cfg));
        //     registry.register_data_provider(ib);
        // }

        // 3. Restore active provider selection from config.
        if let Some(ref prov_cfg) = config.providers {
            if let Some(ref saved_name) = prov_cfg.active_data {
                let names = registry.data_provider_names();
                if let Some(idx) = names.iter().position(|n| *n == saved_name.as_str()) {
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

    /// Attempt to open a DuckDB store. Returns None on failure (graceful fallback).
    fn try_open_store(db_path: &Path, config: &AppConfig) -> Option<midas_store::DbHandle> {
        // Ensure parent directory exists.
        if let Some(parent) = db_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    path = %parent.display(),
                    error = %e,
                    "failed to create store directory"
                );
                return None;
            }
        }

        let store_config = midas_store::StoreConfig {
            path: Some(db_path.to_path_buf()),
            memory_limit_mb: config.store.memory_limit_mb,
            threads: config.store.threads,
        };

        // DbHandle::open() is synchronous and spawns the actor thread.
        // Connection is lazy (first command), so this never fails here.
        // Actual failure will surface on first query, handled by
        // CachingProvider's graceful fallback.
        Some(midas_store::DbHandle::open(store_config))
    }
}
```

---

## 7. Config Save Integration

### build_config() Changes

The existing `build_config()` method in `app/persistence.rs` must be updated
to include the `[providers]` section:

```rust
impl MidasApp {
    pub fn build_config(&self) -> AppConfig {
        AppConfig {
            window: self.build_window_config(),
            theme: self.build_theme_config(),
            charts: self.build_chart_configs(),
            levels: self.build_level_configs(),
            watchlists: self.build_watchlist_configs(),
            panel_order: self.build_panel_order(),
            store: self.build_store_config(),
            // NEW:
            providers: Some(ProviderConfig {
                active_data: Some(
                    self.providers.active_data().name().to_string()
                ),
                active_broker: self
                    .providers
                    .active_broker()
                    .map(|b| b.name().to_string()),
            }),
        }
    }
}
```

### build_store_config() Note

The `build_store_config()` method remains unchanged. The `[store]` section
controls DuckDB configuration (path, memory, threads). The `[providers]`
section controls which provider is active. They are orthogonal:

- `[store]` answers: "How is DuckDB configured?"
- `[providers]` answers: "Which provider is selected?"

---

## 8. View Layer Changes

### Toolbar Provider Dropdown

Add a provider selector to the toolbar (in `app/views.rs`):

```rust
/// Build the provider dropdown for the toolbar.
fn provider_dropdown(&self) -> Element<Message> {
    let names = self.providers.data_provider_names();
    let active_idx = self.providers.active_data_index();

    // Only show the dropdown if there are multiple providers.
    if names.len() <= 1 {
        // Single provider: show as static text.
        return text(names.first().map(|s| s.as_str()).unwrap_or("No provider"))
            .size(12)
            .into();
    }

    pick_list(
        names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        Some(names[active_idx].to_string()),
        |selected| {
            Message::DataProviderSelected(selected)
        },
    )
    .text_size(12)
    .into()
}
```

Note: The exact widget type depends on iced's API. `pick_list` is the
standard iced dropdown. If a custom styled dropdown is needed, it can be
built from `button` + `container` with overlay positioning.

### Status Bar Provider Indicator

Show the active provider name and connection status in the status bar:

```rust
fn status_bar_provider_text(&self) -> String {
    let provider = self.providers.active_data();
    let status = if provider.is_connected() {
        "connected"
    } else {
        "disconnected"
    };
    format!("[{}:{}]", provider.name(), status)
}
```

---

## 9. Backward Compatibility

### Guarantee 1: No [providers] Section

Existing `config.toml` files without a `[providers]` section continue to
work. The `#[serde(default)]` on `AppConfig.providers: Option<ProviderConfig>`
deserializes `None`, which the startup code treats as "use the first
registered provider."

### Guarantee 2: No [store] Section

If `[store]` is absent, `StoreConfig::default()` enables DuckDB with
default settings. This matches the current behavior.

### Guarantee 3: DuckDB Fails to Open

If DuckDB cannot open its database file (permissions, corrupt file, missing
directory), `build_provider_registry()` falls back to raw `TestProvider`.
The app starts normally with identical behavior to pre-DuckDB builds.

### Guarantee 4: All Chart Behavior Unchanged

With a single provider (TestProvider or CachingProvider wrapping TestProvider),
the user experience is identical to the current synchronous flow:

| User Action | Before | After |
|-------------|--------|-------|
| Type "AAPL" + Enter | Data appears same frame | Data appears within 1-2 frames |
| Change timeframe | Data appears same frame | Data appears within 1-2 frames |
| App restart with saved charts | Data loaded before first paint | Data loaded within 50ms of first paint |
| Gap toggle | Data reloaded same frame | Data appears within 1-2 frames |

The 1-2 frame latency increase (from synchronous to async via `Task::perform`)
is imperceptible. During the 1-frame delay, the chart shows a "Loading..."
state, which is the correct UX for any data loading operation.

### Guarantee 5: Config Round-Trip

The `[providers]` section is only written when providers are registered.
If no providers section exists in the file, none is written (the `Option`
is `None` on first launch, then `Some(...)` after the first save). The file
does not grow unnecessarily.

---

## 10. Migration Checklist

### Files Modified

| File | Changes |
|------|---------|
| `midas-core/src/config.rs` | Add `ProviderConfig` struct, add `providers` field to `AppConfig` |
| `midas-core/src/provider.rs` | **New:** `DataProvider` trait, `OrderBroker` trait, `ProviderError`, `CandleBuffer` moved here |
| `midas-core/src/lib.rs` | Add `pub mod provider;`, re-export types |
| `midas-core/Cargo.toml` | Add `async-trait` dependency |
| `midas-feed/src/lib.rs` | Add `pub mod test_provider;`, re-export `TestProvider` |
| `midas-feed/src/test_provider.rs` | **New:** `TestProvider` struct implementing `DataProvider` |
| `midas-feed/Cargo.toml` | Add `async-trait`, `parking_lot` dependencies |
| `midas-store/src/caching_provider.rs` | **New:** `CachingProvider` implementing `DataProvider` |
| `midas-store/src/lib.rs` | Add `pub mod caching_provider;`, re-export `CachingProvider` |
| `midas-store/Cargo.toml` | No new deps (already depends on `midas-core`) |
| `midas-app/src/registry.rs` | **New:** `ProviderRegistry` struct |
| `midas-app/src/app.rs` | Major rewrite: remove `test_data`/`store`, add `providers`, rewrite handlers |
| `midas-app/src/app/persistence.rs` | Add `providers` to `build_config()` |
| `midas-app/src/app/views.rs` | Add provider dropdown to toolbar |
| `midas-app/src/main.rs` | No change |
| `midas-app/Cargo.toml` | No change (already depends on midas-feed and midas-store) |

### Files Deleted

None. `TestDataProvider` in `midas-feed/src/testdata.rs` is retained (it is
wrapped by `TestProvider`, not replaced).

### New Dependencies

| Crate | Dependency | Version |
|-------|-----------|---------|
| `midas-core` | `async-trait` | `0.1` |
| `midas-feed` | `async-trait`, `parking_lot` | `0.1`, workspace |

---

## 11. Sequence Diagrams

### Normal Data Load (Post-Migration)

```
User              MidasApp::update()           Tokio Runtime          DataProvider
 |                       |                           |                      |
 |-- "AAPL" + Enter ---->|                           |                      |
 |                       |                           |                      |
 |                       |-- chart.load_state = Loading                     |
 |                       |-- status = "Loading AAPL..."                     |
 |                       |                           |                      |
 |                       |-- Task::perform ---------->|                     |
 |                       |   (Arc<dyn DataProvider>)  |                     |
 |                       |                           |-- get_candles() ---->|
 |                       |                           |                      |
 |                       |                           |<-- Ok(CandleBuffer) -|
 |                       |                           |                      |
 |                       |<-- DataLoaded(id, Ok) ----|                      |
 |                       |                           |                      |
 |                       |-- apply_candle_data(chart, buffer, true)         |
 |                       |-- status = "AAPL: 500 candles at 1d"            |
 |                       |                           |                      |
 |<-- chart renders -----|                           |                      |
```

### Provider Switch

```
User              MidasApp::update()           Tokio Runtime
 |                       |                           |
 |-- Select "IB TWS" --->|                           |
 |                       |                           |
 |                       |-- set_active_data(1)      |
 |                       |-- for each chart:         |
 |                       |     load_state = Loading  |
 |                       |     Task::perform ------->| (x N)
 |                       |                           |
 |                       |-- status = "Switched to IB TWS (reloading 4 charts)"
 |                       |                           |
 |                       |<-- DataLoaded(id1, Ok) ---|
 |                       |-- apply_candle_data(...)   |
 |                       |                           |
 |                       |<-- DataLoaded(id2, Ok) ---|
 |                       |-- apply_candle_data(...)   |
 |                       |     ...                   |
 |                       |                           |
 |<-- all charts render -|                           |
```

### Startup Restore

```
MidasApp::new()          Tokio Runtime          CachingProvider        DuckDB
 |                            |                       |                  |
 |-- build_provider_registry  |                       |                  |
 |-- for each saved chart:    |                       |                  |
 |     load_state = Loading   |                       |                  |
 |     Task::perform -------->|                       |                  |
 |                            |-- get_candles() ----->|                  |
 |                            |                       |-- query cache -->|
 |                            |                       |<-- hit/miss -----|
 |                            |                       |                  |
 |                            |<-- Ok(buffer) --------|                  |
 |                            |                       |                  |
 |<-- DataRestoredFromStartup |                       |                  |
 |-- apply_candle_data(chart, buffer, false)           |                  |
 |   (camera NOT reset, saved position preserved)      |                  |
```

---

## 12. Error Handling Matrix

| Error Source | Where Caught | User Impact | Recovery |
|-------------|-------------|-------------|----------|
| `ProviderError::NotConnected` | `DataLoaded` handler | Chart shows "disconnected" | Switch to TestProvider |
| `ProviderError::UnknownSymbol` | `DataLoaded` handler | Chart shows "not found" | User types different symbol |
| `ProviderError::UnsupportedTimeframe` | `DataLoaded` handler | Chart shows error message | User selects different timeframe |
| `ProviderError::Internal` | `DataLoaded` handler | Chart shows error message | Retry (re-submit symbol) |
| DuckDB query failure | `CachingProvider` (logged) | None (falls through) | Automatic |
| DuckDB write failure | `CachingProvider` (logged) | None (data still shown) | Automatic |
| DuckDB open failure | `build_provider_registry` | None (falls back to raw) | Automatic |
| Lock poisoned (TestProvider) | `get_candles` returns Err | Chart shows error | Restart app |
| Tokio task panic | `DataLoaded` never arrives | Chart stuck on "Loading" | Timeout + retry (future) |

### Loading State Transitions

```
Empty ──────── PanelSymbolSubmitted ──────> Loading
Loading ────── DataLoaded(Ok) ────────────> Loaded
Loading ────── DataLoaded(Err) ───────────> Error
Loaded ─────── PanelSymbolSubmitted ──────> Loading  (new symbol)
Loaded ─────── PanelTimeframeSelected ───> Loading  (new timeframe)
Loaded ─────── DataProviderSelected ─────> Loading  (provider switch)
Error ──────── PanelSymbolSubmitted ──────> Loading  (retry)
```

---

## 13. Performance Considerations

### Async Overhead for TestProvider

The `TestProvider` generates data synchronously (~2ms for 500 candles at D1).
Wrapping it in `Task::perform` adds:

- tokio task spawn overhead: ~1us
- channel send/receive: ~1us
- iced message dispatch: ~1us

Total overhead: ~3us. Negligible compared to the 2ms generation time.

### Startup Latency

**Before (synchronous):** 20 charts x 2ms = ~40ms total. All charts render
on the first frame.

**After (async):** 20 charts spawned in parallel. TestProvider behind a
Mutex serializes generation (~40ms total), but the first chart renders as
soon as its task completes (~2ms). All 20 charts render within ~50ms.
The user sees charts appearing one by one in rapid succession rather than
all at once. This is a better UX for slow providers (IB, network).

With `CachingProvider` and a warm DuckDB cache: 20 cache hits x ~0.5ms
= ~10ms total. Faster than the synchronous path.

### Memory Impact

No change. Providers are `Arc`'d reference-counted pointers. The actual
data (`CandleBuffer`) was already `Arc`'d. The `ProviderRegistry` itself
is ~64 bytes (two Vecs + two indices).
