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

pub mod migrations;

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

/// Current on-disk schema version. Bumped whenever an
/// [`AppConfig`] field rename, type change, or migration step lands;
/// the [`migrations`] module owns the v_n → v_{n+1} chain.
///
/// Configs without a `version` field deserialize as v1 (the
/// pre-versioning schema) — see [`default_config_version`]. Older
/// versions are walked forward to [`CURRENT_CONFIG_VERSION`] on
/// load; the migrated form is then saved back, stamping the
/// current value.
pub const CURRENT_CONFIG_VERSION: u32 = 2;

/// Default for `AppConfig::version` when the field is missing on
/// disk. v1 = pre-versioning (anything written before the
/// `version` field landed).
fn default_config_version() -> u32 {
    1
}

/// Root application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// On-disk schema version. Always equals
    /// [`CURRENT_CONFIG_VERSION`] for in-memory configs after
    /// `load`; older files round-trip through the migration chain.
    #[serde(default = "default_config_version")]
    pub version: u32,
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
    /// Order-blotter panel configurations, persisted across sessions.
    ///
    /// Legacy: retained only as migration input.
    /// [`migrations::migrate_order_blotters_to_account_panels`] clears this
    /// vec and appends equivalent entries to [`Self::account_panels`] on
    /// first load. New writes leave it empty, and `skip_serializing_if`
    /// drops it from the on-disk TOML once the migration completes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_blotters: Vec<OrderBlotterConfig>,
    /// Account-panel configurations (tabbed Positions / Orders / History /
    /// Recents). Replaces the legacy `order_blotters` list.
    #[serde(default)]
    pub account_panels: Vec<AccountPanelConfig>,
    /// Recent-Instruments MRU — just the symbol strings.
    ///
    /// Timestamps are deliberately NOT persisted; they're session-only
    /// and used to render "N min ago" suffixes in the Recents tab. On
    /// reload the in-app `last_seen` falls back to `None` and the UI
    /// renders `"—"`. The app caps this list at its `MAX_RECENTS`
    /// constant (currently 20) before writing.
    #[serde(default)]
    pub recent_symbols: Vec<String>,
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
    /// Broker backend selection. Default: [`BrokerBackend::Sim`] — on
    /// a fresh install, `cargo run -p midas-app` auto-spawns
    /// `midas-ib-sim-server` and connects to it so the app works
    /// out of the box with zero config. See [`BrokerConnectionConfig`]
    /// for the backend-specific fields.
    #[serde(default)]
    pub broker: BrokerConnectionConfig,
    /// Chart-view-store schema stamp — mirrors
    /// `midas-app::chart_view::CURRENT_CHART_VIEW_STORE_SCHEMA`.
    ///
    /// Persisted so the chart-transition rollback story (R6) can tell
    /// a pre-migration layout (v1-only) apart from a migrated layout
    /// (v1+v2 dual-write). Unknown / missing defaults to `0`; the
    /// in-memory store treats that as "never written" and boots with
    /// the default stamp.
    ///
    /// Slice 9c bumps this to `3` when the v1 writes retire.
    #[serde(default)]
    pub chart_view_store_schema: u32,
    /// Experimental flags. Reserved for risk mitigation — a single
    /// config edit can revert behaviour without a binary rollback.
    /// See [`ExperimentalFlags`] for the list of toggles.
    #[serde(default)]
    pub experimental: ExperimentalFlags,
}

/// Experimental / kill-switch flags. Each field is a single toggle that
/// flips a feature back to its pre-feature behaviour without a binary
/// rollback. Default values match the user-visible pre-feature state,
/// so an existing config without `[experimental]` loads unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperimentalFlags {
    /// Anchored Volume Profile global kill-switch. When `true`, both
    /// chart backends force `Anchor = Viewport` regardless of any
    /// per-chart `volume_profile.anchor`. Per-chart settings on disk
    /// are preserved, so toggling this flag off restores the user's
    /// choices.
    ///
    /// The override is enforced at the two `midas-app` assembly sites
    /// that already see both `AppConfig` and per-chart state:
    /// - Legacy stack: `ChartInput.effective_vp_anchor` is set to
    ///   `Viewport` when this flag is `true`.
    /// - New stack: `session_chart::scene_builder` computes the
    ///   effective scene anchor with the same conditional and passes
    ///   it into `VolumeProfileLayer`.
    ///
    /// `midas-chart` and `midas-scene` are leaf crates and never read
    /// `AppConfig` directly (Architecture Rule 9).
    #[serde(default)]
    pub disable_anchored_vp: bool,
}

/// Which chart rendering backend a panel uses.
///
/// Added in chart-transition slice 9a. Each [`ChartConfig`] persists
/// an optional value; unset (the on-disk default) means `Legacy`.
///
/// The enum is **feature-independent** — it always deserializes even
/// when the binary is built without `--features session_chart`. The
/// dispatch in `midas-app::app::views` handles the mismatch: if the
/// build lacks `session_chart` but the config selects
/// [`ChartBackend::New`], the panel falls back to legacy rendering
/// with a `tracing::warn!` on first encounter (slice 9a, plan
/// Scenario 9). No panic, no silent drop.
///
/// Serialized as a lower-case string in TOML so hand-edited configs
/// stay readable: `backend = "new"` or `backend = "legacy"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartBackend {
    /// Legacy chart stack (`midas-chart`, `midas-render::ChartScene`,
    /// `Camera2D`). Default until slice 9b flips the app-wide default
    /// to `New` after the 14-day soak.
    #[default]
    Legacy,
    /// Session-aware new stack (`midas-scene`, `midas-axis`,
    /// `midas-bars`). Requires the `session_chart` Cargo feature to
    /// be enabled on `midas-app`; otherwise the dispatch falls back
    /// to `Legacy`.
    New,
}

/// Which broker backend the app connects to on startup.
///
/// Serialized as the `type` tag inside the `[broker]` TOML table so
/// adding new backends later is an enum variant, not a schema
/// renaming exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrokerBackend {
    /// Default for dev: auto-spawn `midas-ib-sim-server` as a child
    /// process and connect to its TWS port via rust-ibapi. No
    /// real-money risk; works with zero config.
    #[default]
    Sim,
    /// Connect to a running IB paper-trading gateway at
    /// [`BrokerConnectionConfig::host`]:[`BrokerConnectionConfig::port`].
    /// The `allow_live` guard in [`BrokerConnectionConfig::validate`]
    /// protects against accidentally pointing this at port 4001
    /// (the live-money port).
    #[serde(rename = "live_paper")]
    LivePaper,
    /// Connect to a real IB live gateway. Refuses unless
    /// `allow_live = true` is explicitly set AND the port is not a
    /// known paper-trading port (safety lockout).
    Live,
}

/// Per-backend broker connection settings.
///
/// Mirrors the subset of `midas_broker::ConnectionConfig` that the
/// app layer needs to know about. The full broker-engine config
/// (persistence paths, reconnect timers, order defaults, trading
/// limits) lives in the engine and is not user-tunable from here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerConnectionConfig {
    /// Selected backend. Default: [`BrokerBackend::Sim`].
    #[serde(default)]
    pub backend: BrokerBackend,
    /// TWS/Gateway hostname (ignored for `Sim`, which always binds
    /// loopback).
    #[serde(default = "default_broker_host")]
    pub host: String,
    /// Preferred TWS/Gateway port. For `Sim` this is the port the
    /// child will try to bind first; if it's taken,
    /// [`crate::sim_child::allocate_sim_port`] falls back to a free
    /// port in 7498..7600.
    #[serde(default = "default_broker_port")]
    pub port: u16,
    /// Unique client identifier for the ibapi connection.
    #[serde(default = "default_broker_client_id")]
    pub client_id: i32,
    /// Must be `true` to allow connecting to a known live-money
    /// gateway port. Default `false`.
    #[serde(default)]
    pub allow_live: bool,
}

fn default_broker_host() -> String {
    "127.0.0.1".to_string()
}

fn default_broker_port() -> u16 {
    7498
}

fn default_broker_client_id() -> i32 {
    1
}

impl Default for BrokerConnectionConfig {
    fn default() -> Self {
        Self {
            backend: BrokerBackend::default(),
            host: default_broker_host(),
            port: default_broker_port(),
            client_id: default_broker_client_id(),
            allow_live: false,
        }
    }
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

/// Volume Profile anchor mode — how the histogram is segmented.
///
/// `Viewport` (default) is the legacy single-histogram mode. The four
/// per-period modes (`Daily`/`Weekly`/`Monthly`/`Yearly`) split the
/// visible range into one profile per calendar period, left-anchored
/// at each period's first candle. See `plan/volume-profile-anchored/`
/// for the full design.
///
/// **Duplicated by design** — a parallel render-time copy lives in
/// `midas_scene::VolumeProfileAnchor` (no serde). Architecture Rule 9
/// forbids the root crate `midas-scene` depending on this desktop
/// crate; the `From`/`Into` bridge between the two enums lives in
/// `midas-app`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum VolumeProfileAnchor {
    /// Single histogram across the entire visible viewport. Matches
    /// pre-feature behaviour.
    #[default]
    Viewport,
    /// One histogram per calendar day (per the chart's exchange
    /// timezone — ET for stocks, UTC for crypto on the new stack).
    Daily,
    /// One histogram per ISO week (Mon-start).
    Weekly,
    /// One histogram per calendar month.
    Monthly,
    /// One histogram per calendar year.
    Yearly,
    /// Forward-compat sink: a downgraded binary loading a config with
    /// a future-version anchor falls back here. Render code treats
    /// `Unknown` exactly like `Viewport`.
    #[serde(other)]
    Unknown,
}

/// Per-chart Volume Profile configuration.
///
/// `width_fraction` is the fraction of the period's pixel span the
/// largest bar consumes (clamped to `[0.05, 1.0]`). The `value_area_*`
/// fields are reserved for v2 (Slice 6 P4); v1 render code ignores
/// them. See `plan/volume-profile-anchored/01-slice-1-...md` for the
/// schema decisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeProfileSettings {
    /// Anchor mode — see [`VolumeProfileAnchor`]. Default `Viewport`.
    #[serde(default)]
    pub anchor: VolumeProfileAnchor,
    /// Fraction of period pixel span the largest bar consumes.
    /// Clamped to `[0.05, 1.0]` by [`Self::sanitized`].
    #[serde(default = "default_vp_width_fraction")]
    pub width_fraction: f32,
    /// RESERVED for v2 (Slice 6 P4 — value-area rendering). Default
    /// `false`. v1 render code ignores this field; persisted so a
    /// future-version binary won't drop user state.
    #[serde(default)]
    pub show_value_area: bool,
    /// RESERVED for v2. Volume fraction defining the value area.
    /// Default `0.70` (TradingView convention). Clamped to
    /// `[0.10, 0.95]` by [`Self::sanitized`].
    #[serde(default = "default_vp_value_area_pct")]
    pub value_area_pct: f32,
}

fn default_vp_width_fraction() -> f32 {
    // Per-period histogram width as a fraction of the period's pixel
    // span. New stack default at
    // `midas-scene::layers::volume_profile::VolumeProfileConfig::default`
    // is 0.7; legacy stack matches so anchored bars are visible by
    // default rather than rendering as 6–13 px slivers (the 0.25
    // value originally copy-pasted from the legacy single-viewport
    // 25%-of-viewport-width fraction, which is a different concept).
    0.7
}

fn default_vp_value_area_pct() -> f32 {
    0.70
}

impl Default for VolumeProfileSettings {
    fn default() -> Self {
        Self {
            anchor: VolumeProfileAnchor::Viewport,
            width_fraction: default_vp_width_fraction(),
            show_value_area: false,
            value_area_pct: default_vp_value_area_pct(),
        }
    }
}

impl VolumeProfileSettings {
    /// Return a copy with out-of-range float fields clamped to their
    /// valid ranges. Called both on save (`build_config`) and on load
    /// (`restore_panel`) — belt & braces — so a malformed manual
    /// edit doesn't survive even one save→load cycle.
    pub fn sanitized(&self) -> Self {
        Self {
            anchor: self.anchor,
            width_fraction: self.width_fraction.clamp(0.05, 1.0),
            show_value_area: self.show_value_area,
            value_area_pct: self.value_area_pct.clamp(0.10, 0.95),
        }
    }
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
    /// Volume Profile settings (anchor mode + render knobs). New in
    /// the `volume-profile-anchored` plan. Defaults preserve
    /// pre-feature behaviour: `Viewport` anchor + 0.25 width.
    #[serde(default)]
    pub volume_profile: VolumeProfileSettings,
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
    /// Bound symbol key from the symbol-link color group.
    ///
    /// Persisted so the binding survives restart. If absent, falls back
    /// to the legacy `symbol` field during restoration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_symbol: Option<String>,
    /// Chart rendering backend selection (chart-transition slice 9a).
    ///
    /// `None` (the on-disk default) means "follow the app default",
    /// which is currently [`ChartBackend::Legacy`]. Slice 9b flips the
    /// default to [`ChartBackend::New`] after the 14-day soak; at that
    /// point existing configs without this field keep rendering with
    /// the new default (because `None` maps to whatever
    /// `ChartBackend::default()` returns).
    ///
    /// The enum always deserializes, even when the binary is built
    /// without `--features session_chart`; the feature-gate × config
    /// mismatch is handled in the app's dispatch layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<ChartBackend>,
    /// Whether to fetch extended-hours bars from the broker for this
    /// chart. Drives the `use_rth` flag on
    /// `MarketDataRouter::historical_bars` (negated): `true` here →
    /// `use_rth = false`, so pre / post bars reach the chart. Default
    /// `true` (ETH ships on by default per the plan); existing configs
    /// missing the field load with the default.
    #[serde(default = "default_true")]
    pub show_extended_hours: bool,
    /// Whether the chart should paint the pre/post-market session band
    /// overlay behind the candles. Drives the `compute_session_bands`
    /// pass in `midas-chart`. Independent from `show_extended_hours`
    /// — a user can fetch ETH bars but suppress the tint, or vice
    /// versa. Default `true`.
    #[serde(default = "default_true")]
    pub show_extended_hours_bands: bool,
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
    /// Bracket chart toggle state: `"BUY"`, `"SELL"`, or `"NONE"`.
    /// Persisted so the toggle survives app restarts.
    /// Defaults to `None` if missing from config (backward compat).
    #[serde(default)]
    pub bracket_active: Option<String>,
    /// Bound symbol key from the symbol-link color group.
    ///
    /// Persisted so the binding survives restart. If absent, falls back
    /// to the legacy `symbol` field during restoration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_symbol: Option<String>,
}

impl Default for OrderPanelConfig {
    fn default() -> Self {
        Self {
            symbol: String::new(),
            side: default_order_side(),
            quantity: default_order_quantity(),
            symbol_link: LinkMode::default(),
            bracket_active: None,
            bound_symbol: None,
        }
    }
}

/// Default order side for configs missing the field.
fn default_order_side() -> String {
    "BUY".to_string()
}

/// Order-blotter panel configuration for session persistence.
///
/// Legacy: retained as migration input only. New configs use
/// [`AccountPanelConfig`]; `migrate_order_blotters_to_account_panels`
/// converts `order_blotters` entries on first load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBlotterConfig {
    /// User-visible panel name.
    #[serde(default = "default_order_blotter_name")]
    pub name: String,
    /// Column widths in logical pixels (persisted for session restore).
    #[serde(default)]
    pub column_widths: Vec<f32>,
    /// Symbol-link group — row clicks broadcast to panels sharing
    /// the same colour. `Unlinked` (default) = no broadcast.
    #[serde(default)]
    pub symbol_link: LinkMode,
    /// Column IDs the user has hidden via the column-selector popup.
    /// Stored as strings (the underlying `ColumnId(&'static str)`)
    /// so future column additions are forward-compat.
    #[serde(default)]
    pub hidden_columns: Vec<String>,
}

impl Default for OrderBlotterConfig {
    fn default() -> Self {
        Self {
            name: default_order_blotter_name(),
            column_widths: Vec::new(),
            symbol_link: LinkMode::default(),
            hidden_columns: Vec::new(),
        }
    }
}

fn default_order_blotter_name() -> String {
    "Orders".to_string()
}

// ── Account panel config ─────────────────────────────────────────────

/// Which Account-panel tab is currently active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountTab {
    /// Positions tab.
    Positions,
    /// Orders (working / all) tab.
    #[default]
    Orders,
    /// Trade History tab (terminal orders).
    TradeHistory,
    /// Recent Instruments tab.
    Recents,
}

/// Persisted state for the Orders tab inside an Account panel.
///
/// Structurally equal to the legacy [`OrderBlotterConfig`]; the
/// migration step copies fields 1:1 when converting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrdersTabConfig {
    /// Column widths in logical pixels (persisted for session restore).
    #[serde(default)]
    pub column_widths: Vec<f32>,
    /// Symbol-link group — row clicks broadcast to panels sharing
    /// the same colour. `Unlinked` (default) = no broadcast.
    #[serde(default)]
    pub symbol_link: LinkMode,
    /// Column IDs the user has hidden via the column-selector popup.
    /// Stored as strings (the underlying `ColumnId(&'static str)`)
    /// so future column additions are forward-compat.
    #[serde(default)]
    pub hidden_columns: Vec<String>,
}

/// Account-panel configuration for session persistence.
///
/// Wraps the former Orders blotter + future Positions / History /
/// Recents tab state. v1: only Orders persists per-tab widths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountPanelConfig {
    /// User-visible panel name. Defaults to `"Account"`.
    #[serde(default = "default_account_panel_name")]
    pub name: String,
    /// Which tab was active when the panel was last rendered.
    #[serde(default)]
    pub active_tab: AccountTab,
    /// Orders tab state (column widths, link mode, hidden columns).
    #[serde(default)]
    pub orders: OrdersTabConfig,
}

impl Default for AccountPanelConfig {
    fn default() -> Self {
        Self {
            name: default_account_panel_name(),
            active_tab: AccountTab::default(),
            orders: OrdersTabConfig::default(),
        }
    }
}

fn default_account_panel_name() -> String {
    "Account".to_string()
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
    /// Favourite level: `0` = off, `1..=5` = graded silver→gold.
    ///
    /// Stored as a u8 but the deserializer accepts an old-config bool
    /// (`true` → `1`, `false` → `0`) so configs written before the
    /// graded star landed keep loading.
    #[serde(default, deserialize_with = "deserialize_favorite_level")]
    pub favorite: u8,
}

fn deserialize_favorite_level<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FavIn {
        Bool(bool),
        Level(u8),
    }
    match FavIn::deserialize(deserializer)? {
        FavIn::Bool(b) => Ok(u8::from(b)),
        FavIn::Level(n) if n <= 5 => Ok(n),
        FavIn::Level(n) => Err(D::Error::custom(format!(
            "favorite level out of range 0..=5: {n}"
        ))),
    }
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
    /// An order-blotter panel — index into `AppConfig::order_blotters`.
    ///
    /// Legacy variant: on load, any `OrderBlotter` slots are converted to
    /// `Account` slots referencing a migrated entry in `account_panels`.
    #[serde(rename = "order_blotter")]
    OrderBlotter {
        /// Index into `AppConfig::order_blotters`.
        order_blotter_index: usize,
    },
    /// An Account panel (tabbed) — index into `AppConfig::account_panels`.
    #[serde(rename = "account")]
    Account {
        /// Index into `AppConfig::account_panels`.
        account_panel_index: usize,
    },
    /// Forward-compatibility catch-all for unknown panel types.
    /// Matches the pattern on `LayoutNode` — prevents deserialization
    /// failure if an older binary loads a config written by a newer one.
    /// Restoration code treats `Unknown` as a no-op slot.
    #[serde(other)]
    Unknown,
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
    /// An order-blotter pane — index into `AppConfig::order_blotters`.
    ///
    /// Legacy variant: converted to `Account` on load by the migration step.
    #[serde(rename = "order_blotter")]
    OrderBlotter {
        /// Index into `AppConfig::order_blotters`.
        order_blotter_index: usize,
    },
    /// An Account pane — index into `AppConfig::account_panels`.
    #[serde(rename = "account")]
    Account {
        /// Index into `AppConfig::account_panels`.
        account_panel_index: usize,
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
            // Fresh configs start at the current version — they were
            // born with the latest schema, so no migration applies.
            version: CURRENT_CONFIG_VERSION,
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
            order_blotters: Vec::new(),
            account_panels: Vec::new(),
            recent_symbols: Vec::new(),
            panel_order: Vec::new(),
            layout_tree: Vec::new(),
            store: StoreConfig::default(),
            providers: None,
            broker: BrokerConnectionConfig::default(),
            chart_view_store_schema: 0,
            experimental: ExperimentalFlags::default(),
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
        // Pre-versioning structural migration. Stays out of the
        // version chain because it's structurally idempotent (skipped
        // when `levels` already populated) and predates the version
        // field; existing v1 files may or may not have it applied.
        migrate_levels(&mut config);

        // Walk v_n → v_{n+1} → … → CURRENT_CONFIG_VERSION. The
        // framework reports back which steps ran so we can write a
        // single backup file regardless of how many versions the
        // caller jumped.
        let initial_version = config.version;
        let steps = migrations::migrate_to_current(&mut config);
        if !steps.is_empty() {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let backup = path.with_file_name(format!(
                "{file_name}.bak-v{initial_version}-to-v{}",
                CURRENT_CONFIG_VERSION
            ));
            if !backup.exists() {
                std::fs::copy(path, &backup).map_err(ConfigError::Io)?;
                tracing::info!(
                    "Migrated config v{initial_version} → v{}: {}; backup at {}",
                    CURRENT_CONFIG_VERSION,
                    steps.join(", "),
                    backup.display()
                );
            } else {
                tracing::warn!(
                    "Migrated config v{initial_version} → v{}: {}; \
                     existing backup at {} preserved",
                    CURRENT_CONFIG_VERSION,
                    steps.join(", "),
                    backup.display()
                );
            }
        }
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
