//! Application configuration types and persistence.
//!
//! `AppConfig` is the root configuration struct, serialized as TOML.
//! It stores window geometry, theme preferences, and per-chart state
//! (symbol, timeframe, horizontal levels, camera position, gap collapsing)
//! so the workspace can be restored across sessions.
//!
//! Writes use atomic file replacement via `tempfile::NamedTempFile` to
//! prevent corruption if the process is interrupted mid-save.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::link::LinkMode;

// ── Error type ───────────────────────────────────────────────────────

/// Errors that can occur during config load/save operations.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read or write the config file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Failed to parse the TOML content.
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    /// Failed to serialize config to TOML.
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

// ── Config structs ───────────────────────────────────────────────────

/// Root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Window size settings.
    pub window: WindowConfig,
    /// Theme settings.
    pub theme: ThemeConfig,
    /// Per-chart configuration, persisted across sessions.
    #[serde(default)]
    pub charts: Vec<ChartConfig>,
    /// Per-ticker horizontal levels, keyed by symbol.
    ///
    /// Each key is a ticker symbol (e.g. `"AAPL"`) and the value is the
    /// list of levels for that ticker. Serialized as `[[levels.AAPL]]` etc.
    #[serde(default)]
    pub levels: HashMap<String, Vec<LevelConfig>>,
    /// Watchlist configurations, persisted across sessions.
    #[serde(default)]
    pub watchlists: Vec<WatchlistConfig>,
    /// Order panel configurations, persisted across sessions.
    #[serde(default)]
    pub order_panels: Vec<OrderPanelConfig>,
    /// Ordered list of panel types in the pane grid, in BTreeMap key order
    /// (pane creation order — NOT spatial position). Save and restore both
    /// use the same iteration order, so the mapping is self-consistent.
    /// Full spatial layout topology is not preserved (same as chart-only restore).
    /// If absent or empty, falls back to charts-only restoration (backward compat).
    #[serde(default)]
    pub panel_order: Vec<PanelSlot>,
    /// Flattened layout tree (pre-order traversal of the pane split tree).
    /// Preserves full topology, axes, and split ratios. If present, takes
    /// priority over `panel_order` during restoration.
    #[serde(default)]
    pub layout_tree: Vec<LayoutNode>,
    /// DuckDB persistent cache configuration.
    #[serde(default)]
    pub store: StoreConfig,
    /// Active data provider and broker selections.
    #[serde(default)]
    pub providers: Option<ProviderConfig>,
}

/// Window geometry configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowConfig {
    /// Window width in logical pixels.
    pub width: u32,
    /// Window height in logical pixels.
    pub height: u32,
    /// Whether the window is maximized.
    #[serde(default)]
    pub maximized: bool,
    /// Window X position in logical pixels (top-left corner).
    #[serde(default)]
    pub x: Option<i32>,
    /// Window Y position in logical pixels (top-left corner).
    #[serde(default)]
    pub y: Option<i32>,
    /// Width of the monitor the window was last on (for fit validation).
    #[serde(default)]
    pub monitor_width: Option<u32>,
    /// Height of the monitor the window was last on.
    #[serde(default)]
    pub monitor_height: Option<u32>,
}

/// Theme configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Theme mode: `"dark"` or `"light"`.
    pub mode: String,
}

/// Per-chart configuration for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartConfig {
    /// Ticker symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Timeframe display name (e.g. `"1D"`, `"5m"`).
    pub timeframe: String,
    /// DEPRECATED: Levels migrated to top-level `[levels]` table.
    /// Retained for one-time migration from old config format.
    #[serde(default, skip_serializing)]
    pub levels: Vec<LevelConfig>,
    /// Camera time-axis start (restored on load so user sees same view).
    #[serde(default)]
    pub camera_time_start: Option<f64>,
    /// Camera time-axis end.
    #[serde(default)]
    pub camera_time_end: Option<f64>,
    /// Camera price-axis low.
    #[serde(default)]
    pub camera_price_low: Option<f64>,
    /// Camera price-axis high.
    #[serde(default)]
    pub camera_price_high: Option<f64>,
    /// Whether session gaps are collapsed (index-based X positioning).
    #[serde(default)]
    pub collapse_gaps: bool,
    /// Timeline border position (fraction of viewport for volume area, default 0.20).
    #[serde(default = "default_timeline_border_ratio", alias = "separator_ratio")]
    pub timeline_border_ratio: f32,
    /// Volume bar height multiplier (1.0 = default).
    #[serde(default = "default_volume_scale")]
    pub volume_scale: f32,
    /// Whether Volume Profile overlay is enabled.
    #[serde(default)]
    pub show_volume_profile: bool,
    /// Whether horizontal price levels are visible.
    #[serde(default = "default_true")]
    pub show_levels: bool,
    /// Viewport width at save time (prevents scale distortion on restore).
    #[serde(default)]
    pub viewport_width: Option<u32>,
    /// Viewport height at save time.
    #[serde(default)]
    pub viewport_height: Option<u32>,
    /// Symbol link mode for cross-chart symbol synchronization.
    #[serde(default, skip_serializing_if = "LinkMode::is_unlinked")]
    pub symbol_link: LinkMode,
    /// Timeframe link mode for cross-chart timeframe synchronization.
    #[serde(default, skip_serializing_if = "LinkMode::is_unlinked")]
    pub timeframe_link: LinkMode,
}

/// Serde default for bool fields that should default to `true`.
fn default_true() -> bool {
    true
}

/// Default timeline border ratio for configs missing the field (backward compat).
fn default_timeline_border_ratio() -> f32 {
    0.20
}

/// Default volume scale for configs missing the field (backward compat).
fn default_volume_scale() -> f32 {
    1.0
}

/// Persisted horizontal price level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelConfig {
    /// Price at which the level is drawn.
    pub price: f64,
    /// RGBA color in linear space.
    pub color: [f32; 4],
    /// Line width in logical pixels.
    #[serde(default = "default_line_width")]
    pub line_width: f32,
    /// Optional user label displayed on the chart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Icon identifier ("none", "arrow_up", "arrow_down", "star", "flag", "warning").
    #[serde(default = "default_icon")]
    pub icon: String,
    /// Whether this level is locked (prevents drag and delete).
    #[serde(default)]
    pub locked: bool,
}

/// Order panel configuration for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderPanelConfig {
    /// Ticker symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Order side: `"BUY"` or `"SELL"`.
    #[serde(default = "default_order_side")]
    pub side: String,
    /// Quantity input value (string for text input; parsed on submit).
    #[serde(default = "default_order_quantity")]
    pub quantity: String,
    /// Symbol link mode for cross-panel symbol synchronization.
    #[serde(default)]
    pub symbol_link: LinkMode,
}

impl Default for OrderPanelConfig {
    fn default() -> Self {
        Self {
            symbol: String::new(),
            side: default_order_side(),
            quantity: default_order_quantity(),
            symbol_link: LinkMode::default(),
        }
    }
}

/// Default order side for configs missing the field.
fn default_order_side() -> String {
    "BUY".to_string()
}

/// Default order quantity for configs missing the field.
fn default_order_quantity() -> String {
    "100".to_string()
}

/// Watchlist configuration for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistConfig {
    /// User-defined name for the watchlist.
    pub name: String,
    /// Tickers in the watchlist.
    #[serde(default)]
    pub tickers: Vec<WatchlistTickerConfig>,
    /// Symbol link mode for cross-chart symbol synchronization.
    #[serde(default)]
    pub symbol_link: LinkMode,
    /// Column widths in logical pixels (persisted for session restore).
    #[serde(default)]
    pub column_widths: Vec<f32>,
}

/// A single ticker entry within a watchlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistTickerConfig {
    /// Ticker symbol (e.g. `"AAPL"`).
    pub symbol: String,
    /// Whether this ticker is marked as a favorite.
    #[serde(default)]
    pub favorite: bool,
}

/// Records the type of panel in a pane position for layout restoration.
///
/// Used in `AppConfig::panel_order` to reconstruct the pane grid with
/// the correct mix of chart and watchlist panels.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PanelSlot {
    /// A chart panel — index into `AppConfig::charts`.
    Chart {
        /// Index into `AppConfig::charts`.
        chart_index: usize,
    },
    /// A watchlist panel — index into `AppConfig::watchlists`.
    Watchlist {
        /// Index into `AppConfig::watchlists`.
        watchlist_index: usize,
    },
    /// An order panel — index into `AppConfig::order_panels`.
    #[serde(rename = "order_panel")]
    OrderPanel {
        /// Index into `AppConfig::order_panels`.
        order_panel_index: usize,
    },
}

/// Flattened layout tree node for pane grid topology persistence.
///
/// Stored as a pre-order traversal of the binary split tree. A `Split`
/// node's two children are the next two subtrees in the array. Leaf
/// nodes (`Chart`/`Watchlist`) terminate a branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutNode {
    /// A binary split in the pane grid.
    Split {
        /// `"horizontal"` or `"vertical"`.
        axis: String,
        /// Split ratio in \[0.0, 1.0\].
        ratio: f32,
    },
    /// A chart pane — index into `AppConfig::charts`.
    Chart {
        /// Index into `AppConfig::charts`.
        chart_index: usize,
    },
    /// A watchlist pane — index into `AppConfig::watchlists`.
    Watchlist {
        /// Index into `AppConfig::watchlists`.
        watchlist_index: usize,
    },
    /// An order panel pane — index into `AppConfig::order_panels`.
    #[serde(rename = "order_panel")]
    OrderPanel {
        /// Index into `AppConfig::order_panels`.
        order_panel_index: usize,
    },
    /// Forward-compatibility catch-all for unknown panel types.
    /// Prevents deserialization failure if a newer config format is loaded.
    #[serde(other)]
    Unknown,
}

/// Configuration for the DuckDB persistent cache store.
///
/// Serialized as the `[store]` section in `config.toml`. Existing configs
/// without `[store]` get defaults via `#[serde(default)]` on `AppConfig.store`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// Whether the DuckDB cache is enabled.
    #[serde(default = "default_store_enabled")]
    pub enabled: bool,
    /// Path to the DuckDB database file, relative to the data directory.
    #[serde(default = "default_store_path")]
    pub path: String,
    /// Maximum memory DuckDB may use for query processing (MB).
    #[serde(default = "default_store_memory_limit")]
    pub memory_limit_mb: u32,
    /// Number of DuckDB internal threads.
    #[serde(default = "default_store_threads")]
    pub threads: u8,
}

fn default_store_enabled() -> bool {
    true
}
fn default_store_path() -> String {
    "cache.duckdb".into()
}
fn default_store_memory_limit() -> u32 {
    256
}
fn default_store_threads() -> u8 {
    2
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            enabled: default_store_enabled(),
            path: default_store_path(),
            memory_limit_mb: default_store_memory_limit(),
            threads: default_store_threads(),
        }
    }
}

/// Saved provider and broker selections.
///
/// Serialized as the `[providers]` section in `config.toml`. Existing configs
/// without `[providers]` get `None` via `#[serde(default)]` on `AppConfig.providers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Name of the last-active data provider (e.g. `"Test Data"`).
    #[serde(default)]
    pub active_data: Option<String>,
    /// Name of the last-active order broker (e.g. `"IB Paper"`), or `None`.
    #[serde(default)]
    pub active_broker: Option<String>,
}

/// Default line width for levels missing the field (backward compat).
fn default_line_width() -> f32 {
    1.0
}

/// Default icon for levels missing the field (backward compat).
fn default_icon() -> String {
    "none".into()
}

// ── Default implementation ───────────────────────────────────────────

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig {
                width: 1280,
                height: 800,
                ..Default::default()
            },
            theme: ThemeConfig {
                mode: "dark".into(),
            },
            charts: Vec::new(),
            levels: HashMap::new(),
            watchlists: Vec::new(),
            order_panels: Vec::new(),
            panel_order: Vec::new(),
            layout_tree: Vec::new(),
            store: StoreConfig::default(),
            providers: None,
        }
    }
}

// ── Load / Save ──────────────────────────────────────────────────────

impl AppConfig {
    /// Load configuration from a TOML file at `path`.
    ///
    /// Returns `AppConfig::default()` if the file does not exist or is empty.
    /// Returns an error only for genuine I/O or parse failures (e.g. permission
    /// denied, malformed TOML).
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(
                    "Config file not found at {}, using defaults",
                    path.display()
                );
                return Ok(Self::default());
            }
            Err(e) => return Err(ConfigError::Io(e)),
        };

        if content.trim().is_empty() {
            tracing::info!("Config file at {} is empty, using defaults", path.display());
            return Ok(Self::default());
        }

        let mut config: Self = toml::from_str(&content)?;
        migrate_levels(&mut config);
        Ok(config)
    }

    /// Serialize this configuration and atomically write it to `path`.
    ///
    /// Creates parent directories if they do not exist. Uses a temporary
    /// file in the same directory followed by an atomic rename so that a
    /// crash mid-write never leaves a truncated config file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(&mut tmp, content.as_bytes())?;
        tmp.persist(path).map_err(std::io::Error::from)?;
        Ok(())
    }
}

// ── Migration ───────────────────────────────────────────────────────

/// One-time migration: move levels from per-chart `[[charts]].levels`
/// to the top-level `[levels.SYMBOL]` table.
///
/// If `config.levels` already has entries, the migration is skipped
/// (config is already in the new format or was freshly created).
fn migrate_levels(config: &mut AppConfig) {
    if !config.levels.is_empty() {
        return; // already migrated or new config
    }
    // Collect chart data first to avoid borrow conflict
    // (&config.charts immutable vs &mut config.levels).
    let chart_data: Vec<_> = config
        .charts
        .iter()
        .filter(|c| !c.levels.is_empty())
        .map(|c| (c.symbol.clone(), c.levels.clone()))
        .collect();
    for (symbol, levels) in chart_data {
        let ticker_levels = config.levels.entry(symbol).or_default();
        for level in &levels {
            let is_dup = ticker_levels
                .iter()
                .any(|existing| (existing.price - level.price).abs() < 0.0001);
            if !is_dup {
                ticker_levels.push(level.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests;
