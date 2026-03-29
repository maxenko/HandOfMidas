# Phase 4: iced Application Shell — Complete Architecture Plan

> Crate: `midas-app` | Framework: iced 0.14 (wgpu 27 backend, Elm architecture)
> This document defines the complete iced application skeleton: state management, message flow, widget tree, custom GPU rendering integration, subscriptions, theming, and startup sequence.

---

## Table of Contents

1. [Application State (Model)](#1-application-state-model)
2. [Message Enum](#2-message-enum)
3. [Update Function](#3-update-function)
4. [View Function](#4-view-function)
5. [Shader Widget (Chart-to-iced Bridge)](#5-shader-widget-chart-to-iced-bridge)
6. [Subscription System](#6-subscription-system)
7. [Async Data Loading](#7-async-data-loading)
8. [Toolbar Design](#8-toolbar-design)
9. [Theme System](#9-theme-system)
10. [Keyboard Shortcuts](#10-keyboard-shortcuts)
11. [Window Management](#11-window-management)
12. [Startup Sequence](#12-startup-sequence)

---

## 1. Application State (Model)

### Design Philosophy

The top-level `MidasApp` struct owns all application state. Per-chart state lives in `ChartPanel` structs stored in a `HashMap<ChartId, ChartPanel>`. Shared resources (data manager, indicator engine, theme) live at the app level. The active chart concept determines which chart receives toolbar actions and unrouted keyboard events.

### Complete Struct Definitions

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::path::PathBuf;

use midas_core::id::{ChartId, SymbolId};
use midas_core::config::AppConfig;
use midas_data::candle::CandleBuffer;
use midas_data::cache::DataManager;
use midas_data::timeframe::Timeframe;
// Phase 5: midas-indicators crate (not in v1 workspace — stub or remove before Phase 4)
use midas_indicators::engine::IndicatorEngine;
use midas_render::renderer::ChartRenderer;
use midas_chart::camera::Camera2D;

/// Top-level application state. Owns everything.
pub struct MidasApp {
    // ── Chart panels ──────────────────────────────────────────────
    /// All chart panels, keyed by unique ID.
    /// Using HashMap (not Vec) so chart removal is O(1) and IDs are stable.
    charts: HashMap<ChartId, ChartPanel>,

    /// Which chart is "active" (receives toolbar actions, keyboard shortcuts).
    /// None when the workspace is empty.
    active_chart: Option<ChartId>,

    /// Monotonic counter for generating unique ChartIds.
    next_chart_id: u32,

    // ── Layout ────────────────────────────────────────────────────
    /// Binary split tree layout (see window-management-and-chart-layout.md).
    /// Owns the tree structure (root, node IDs, pane IDs).
    layout: WorkspaceLayout,

    /// Maps layout pane identities to chart content identities.
    /// PaneId is the *slot* in the layout tree; ChartId is the *content*.
    /// A PaneId absent from this map means the pane is empty ("Add Chart" prompt).
    pane_chart_map: HashMap<PaneId, ChartId>,

    // ── Sidebar ───────────────────────────────────────────────────
    /// Whether the sidebar (watchlist) is visible.
    sidebar_visible: bool,

    /// Watchlist state.
    watchlist: WatchlistState,

    // ── Toolbar ───────────────────────────────────────────────────
    toolbar: ToolbarState,

    // ── Data layer ────────────────────────────────────────────────
    /// Shared data manager. Handles loading, caching, and binary file I/O.
    /// Wrapped in Arc because async Commands need to reference it.
    data_manager: Arc<DataManager>,

    /// Known symbols for autocomplete (loaded at startup from data directory).
    available_symbols: Vec<String>,

    // ── Indicators ────────────────────────────────────────────────
    // TODO(Phase 5): indicator_engine: IndicatorEngine,
    // The midas-indicators crate is not part of the v1 workspace.
    // Stub this field or remove it before Phase 4 implementation.

    // ── Theme ─────────────────────────────────────────────────────
    theme: MidasTheme,

    // ── Config / persistence ──────────────────────────────────────
    config: AppConfig,
    config_dirty: bool,
    config_save_cooldown: Option<time::Instant>,

    // ── Connection status (for status bar) ────────────────────────
    connection_status: ConnectionStatus,

    // ── Animation state ───────────────────────────────────────────
    /// Whether any chart needs continuous redraws (animation in progress).
    animating: bool,

    // ── Debug overlay ────────────────────────────────────────────
    /// Whether the frame-time / FPS debug overlay is shown (toggled via F3).
    show_frame_overlay: bool,

    // ── Window metadata ───────────────────────────────────────────
    window_size: (u32, u32),
    scale_factor: f64,
}

/// Per-chart panel state. Everything needed to render and interact with one chart.
pub struct ChartPanel {
    pub id: ChartId,

    // ── What this chart displays ──────────────────────────────────
    pub symbol: Option<String>,
    pub timeframe: Timeframe,

    // ── Data ──────────────────────────────────────────────────────
    /// The loaded candle data for this chart. None if no symbol loaded yet.
    pub data: Option<Arc<CandleBuffer>>,

    /// Loading state (for showing spinner).
    pub load_state: LoadState,

    // ── Camera / viewport ─────────────────────────────────────────
    /// Camera2D is a pure coordinate-transform struct (data-space <-> pixel-space).
    /// It does NOT own animation state. Animation targets and flags live here
    /// on ChartPanel so the update() loop can interpolate toward targets
    /// and then write final values into Camera2D each tick.
    pub camera: Camera2D,

    /// Animation targets for smooth Y-axis auto-scaling.
    /// These are owned by ChartPanel, NOT Camera2D. Camera2D only stores the
    /// current price_low/price_high that are actively used for rendering.
    pub target_price_low: f64,
    pub target_price_high: f64,
    pub y_animating: bool,

    // ── Interaction state ─────────────────────────────────────────
    /// Current mouse position within this chart (logical pixels, relative to chart origin).
    pub mouse_position: Option<(f32, f32)>,

    /// Whether the user is currently dragging (panning).
    pub dragging: bool,
    pub drag_start: Option<(f32, f32)>,

    // ── Crosshair ─────────────────────────────────────────────────
    pub crosshair_visible: bool,
    /// Synced crosshair time from another chart (vertical line only).
    pub synced_crosshair_time: Option<f64>,

    // ── Drawing objects ───────────────────────────────────────────
    pub horizontal_levels: Vec<HorizontalLevel>,
    pub selected_level: Option<u64>,

    // ── Indicators ────────────────────────────────────────────────
    pub indicator_configs: Vec<IndicatorConfig>,

    // ── GPU renderer ──────────────────────────────────────────────
    /// The wgpu renderer for this chart. Created lazily on first render.
    /// This is NOT stored here — it lives inside the Shader widget's State.
    /// ChartPanel only holds the data that the renderer needs.

    // ── Sync ──────────────────────────────────────────────────────
    /// Whether this chart is linked to the global TimeAxisController.
    pub time_linked: bool,

    // ── Dirty flags ───────────────────────────────────────────────
    /// Generation counters tracking what changed since last render.
    /// See canonical definition in midas-chart::dirty (see chart-interaction-system.md).
    /// Writers increment counters; the GPU pipeline's DirtyTracker
    /// remembers last-seen generations and compares in Primitive::prepare().
    pub dirty: DirtyFlags,
}

#[derive(Clone, Debug)]
pub enum LoadState {
    /// No data requested yet (empty chart).
    Empty,
    /// Data is being loaded asynchronously.
    Loading,
    /// Data loaded successfully.
    Loaded,
    /// Data load failed with an error message.
    Error(String),
}

/// WorkspaceLayout is the binary split tree defined in
/// window-management-and-chart-layout.md.
///
/// Re-exported from midas-core::layout. The full type is:
///
///   pub struct WorkspaceLayout {
///       pub root: Option<LayoutNode>,
///       next_id: u64,
///   }
///   pub enum LayoutNode {
///       Leaf(LeafNode),       // contains NodeId + Vec<PaneId> tabs
///       Split(SplitNode),     // contains NodeId, Axis, ratio, first, second
///   }
///   pub enum Axis { Horizontal, Vertical }
///   pub struct PaneId(pub u64);   // layout slot identity
///   pub struct NodeId(pub u64);   // tree node identity
///
/// See window-management-and-chart-layout.md Section 2 for the full
/// data model, algorithms (split, close, resize, drag-and-drop),
/// and preset factory methods (preset_1x1, preset_2x2, etc.).
///
/// **PaneId vs ChartId distinction:**
/// - `PaneId` is the *layout identity* — it identifies a slot in the
///   tree and is managed by WorkspaceLayout.
/// - `ChartId` is the *content identity* — it identifies a ChartPanel
///   in the `charts: HashMap<ChartId, ChartPanel>`.
/// - The mapping between them is `pane_chart_map: HashMap<PaneId, ChartId>`.
///   A PaneId absent from the map means the pane slot is empty.
use midas_core::layout::{WorkspaceLayout, LayoutNode, SplitNode, LeafNode, Axis, PaneId, NodeId};

/// Preset layout names for toolbar buttons and keyboard shortcuts.
/// Each variant maps to a factory method on WorkspaceLayout
/// (e.g., LayoutPreset::Grid2x2 calls layout.preset_2x2()).
#[derive(Clone, Debug)]
pub enum LayoutPreset {
    Single,      // preset_1x1()
    SplitH,      // preset_2x1()
    SplitV,      // preset_1x2()
    Grid2x2,     // preset_2x2()
    Grid3x2,     // preset_3x2()
    Grid4x2,     // preset_4x2()
}

pub struct WatchlistState {
    pub symbols: Vec<WatchlistEntry>,
    pub scroll_offset: f32,
}

pub struct WatchlistEntry {
    pub symbol: String,
    pub last_price: Option<f32>,
    pub change_pct: Option<f32>,
}

pub struct ToolbarState {
    /// Current text in the symbol search input.
    pub search_text: String,
    /// Whether the search input is focused.
    pub search_focused: bool,
    /// Filtered autocomplete suggestions (computed on each keystroke).
    pub search_suggestions: Vec<String>,
    /// Index of the highlighted suggestion in the dropdown.
    pub suggestion_index: Option<usize>,
    /// Widget ID for the symbol search text_input (used for programmatic focus).
    pub search_id: iced::widget::text_input::Id,
    /// combo_box state holding available symbols for autocomplete filtering.
    /// This is the canonical reference — use `self.toolbar.search_combo_state` everywhere.
    pub search_combo_state: combo_box::State<String>,
}

#[derive(Clone, Debug)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting { attempt: u32 },
    Error(String),
}
```

### Key Design Decisions

**Why `HashMap<ChartId, ChartPanel>` instead of `Vec<ChartPanel>`**: Chart removal from a Vec requires shifting elements and invalidates indices that other parts of the system may hold. A HashMap with stable ChartId keys makes chart add/remove O(1) and allows the layout system, toolbar, and crosshair sync to reference charts by stable ID.

**Why the GPU renderer is NOT in ChartPanel**: iced's `shader::Program` trait requires that the GPU state (pipeline, buffers) lives inside the Shader widget's associated Pipeline type, which iced manages. ChartPanel provides the data; `view()` builds a `ChartProgram` with a fresh `ChartRenderSnapshot` each frame; then `Program::draw()` reads from `self.snapshot`, builds a `ChartInput`, calls the pure function `compute_chart_scene()`, and wraps the result in a `ChartPrimitive`. `Primitive::prepare()` uploads GPU resources from the `ChartScene`. This avoids lifetime issues with wgpu types, eliminates the `&MidasApp` coupling, and follows iced's ownership model.

**Why `Arc<CandleBuffer>` for data**: The data may be shared between the chart, the indicator engine, and the data manager's cache. Arc avoids copies. The data is immutable once loaded (append-only for real-time updates via a new Arc swap).

**Why `pane_chart_map: HashMap<PaneId, ChartId>`**: The layout tree (WorkspaceLayout) manages geometry via PaneIds. The pane_chart_map decouples layout identity from content identity — you can rearrange charts by updating map entries without touching ChartPanel state. PaneId is assigned by the layout tree; ChartId is assigned by the application. This two-level indirection means a chart can be moved between panes, panes can be empty, and the layout can be rebuilt (e.g., switching presets) without invalidating ChartPanel state.

---

## 2. Message Enum

### Design Philosophy

Messages are the single communication channel in iced's Elm architecture. Every user interaction, async completion, timer tick, and subscription event flows through the `Message` enum. Messages are categorized by domain but live in a single flat enum (not nested) because iced's `update()` pattern-matches on them directly.

```rust
/// All application messages. Every state transition goes through here.
#[derive(Debug, Clone)]
pub enum Message {
    // ═══════════════════════════════════════════════════════════════
    // CHART INTERACTION — Mouse/touch events within a chart panel
    // ═══════════════════════════════════════════════════════════════

    /// Mouse moved within a chart panel. Updates crosshair position.
    /// (chart_id, x_logical, y_logical) — coordinates relative to chart origin.
    ChartMouseMoved(ChartId, f32, f32),

    /// Mouse left a chart panel. Hides crosshair.
    ChartMouseExited(ChartId),

    /// Mouse button pressed in a chart panel.
    /// Used to initiate drag (pan) or select horizontal levels.
    ChartMousePressed(ChartId, MouseButton, f32, f32),

    /// Mouse button released in a chart panel.
    /// Ends drag operations.
    ChartMouseReleased(ChartId, MouseButton, f32, f32),

    /// Scroll wheel within a chart panel.
    /// delta_x for horizontal scroll (time pan), delta_y for zoom.
    ChartScrolled(ChartId, f32, f32),

    /// Double-click in a chart. Creates a horizontal level at the clicked price.
    ChartDoubleClick(ChartId, f32, f32),

    // ═══════════════════════════════════════════════════════════════
    // CHART STATE — Changes to what a chart displays
    // ═══════════════════════════════════════════════════════════════

    /// User selected this chart as the active one (clicked on it, tabbed to it).
    ChartActivated(ChartId),

    /// Symbol changed for a specific chart. Triggers async data load.
    ChartSymbolChanged(ChartId, String),

    /// Timeframe changed for a specific chart. Triggers data reload.
    ChartTimeframeChanged(ChartId, Timeframe),

    /// Toggle time-axis sync for a chart.
    ChartToggleTimeLink(ChartId),

    // ═══════════════════════════════════════════════════════════════
    // HORIZONTAL LEVELS — Drawing/editing price levels
    // ═══════════════════════════════════════════════════════════════

    /// Add a horizontal level at a price on a chart.
    LevelAdd(ChartId, f64),

    /// User started dragging a level (selected it).
    LevelSelected(ChartId, u64),

    /// User dragged a level to a new price.
    LevelMoved(ChartId, u64, f64),

    /// Delete the currently selected level.
    LevelDelete(ChartId, u64),

    /// Deselect any selected level.
    LevelDeselect(ChartId),

    // ═══════════════════════════════════════════════════════════════
    // LAYOUT — Workspace arrangement
    // ═══════════════════════════════════════════════════════════════

    /// Switch to a predefined layout preset. The preset enum names a
    /// factory method on WorkspaceLayout (e.g., preset_2x2). The update
    /// handler calls the factory, rebuilds the tree, and remaps existing
    /// charts to the new PaneIds.
    LayoutPresetSelected(LayoutPreset),

    /// User dragged a split divider to a new position.
    /// (split_node_id, new_ratio) — NodeId identifies the SplitNode.
    LayoutSplitDragged(NodeId, f32),

    /// Add a new chart to the next available empty slot (or expand layout).
    ChartAdd,

    /// Remove a chart from the workspace. Its slot becomes empty.
    ChartRemove(ChartId),

    /// Swap two charts' positions in the layout.
    ChartSwap(ChartId, ChartId),

    // ═══════════════════════════════════════════════════════════════
    // TOOLBAR — Symbol search, timeframe, controls
    // ═══════════════════════════════════════════════════════════════

    /// Text changed in the symbol search input.
    ToolbarSearchChanged(String),

    /// User pressed Enter in the search input. Load the symbol into active chart.
    ToolbarSearchSubmitted,

    /// User selected a suggestion from the autocomplete dropdown.
    ToolbarSuggestionSelected(String),

    /// User pressed Up/Down arrow in search input to navigate suggestions.
    ToolbarSuggestionNavigate(i32),  // -1 = up, +1 = down

    /// Timeframe button clicked on toolbar. Applies to active chart.
    ToolbarTimeframeClicked(Timeframe),

    /// Toggle sidebar (watchlist) visibility.
    ToolbarToggleSidebar,

    // ═══════════════════════════════════════════════════════════════
    // SIDEBAR / WATCHLIST
    // ═══════════════════════════════════════════════════════════════

    /// Clicked a symbol in the watchlist. Loads it into the active chart.
    WatchlistSymbolClicked(String),

    /// Right-clicked a symbol. Opens it in a new chart panel.
    WatchlistSymbolOpenNew(String),

    /// Add a symbol to the watchlist.
    WatchlistAdd(String),

    /// Remove a symbol from the watchlist.
    WatchlistRemove(String),

    // ═══════════════════════════════════════════════════════════════
    // DATA — Async load completions and real-time updates
    // ═══════════════════════════════════════════════════════════════

    /// Async data load completed successfully.
    /// The ChartId identifies which chart requested this data.
    DataLoaded(ChartId, Arc<CandleBuffer>),

    /// Async data load failed.
    DataLoadFailed(ChartId, String),

    /// Symbol list loaded from data directory (startup).
    AvailableSymbolsLoaded(Vec<String>),

    /// A real-time candle update arrived from the feed.
    /// (future — Phase 7)
    RealtimeCandleUpdate(SymbolId, Timeframe, FormingCandle),

    /// A candle closed on the real-time feed.
    /// (future — Phase 7)
    RealtimeCandleClosed(SymbolId, Timeframe, ClosedCandle),

    // ═══════════════════════════════════════════════════════════════
    // ANIMATION / TIMING
    // ═══════════════════════════════════════════════════════════════

    /// 60fps animation tick. Drives Y-axis smooth scaling and any other animations.
    /// Contains the elapsed time since last tick (for frame-rate-independent animation).
    AnimationTick(time::Instant),

    // ═══════════════════════════════════════════════════════════════
    // KEYBOARD — Global shortcuts
    // ═══════════════════════════════════════════════════════════════

    /// A keyboard shortcut was pressed that maps to an action.
    /// We pre-map key events to semantic actions in the subscription.
    KeyboardAction(KeyAction),

    // ═══════════════════════════════════════════════════════════════
    // WINDOW
    // ═══════════════════════════════════════════════════════════════

    /// Window resized. Recalculate chart viewport sizes.
    WindowResized(u32, u32),

    /// Window scale factor changed (moved to different DPI monitor).
    WindowScaleFactorChanged(f64),

    /// Window close requested. Save config and exit.
    WindowCloseRequested,

    // ═══════════════════════════════════════════════════════════════
    // CONFIG
    // ═══════════════════════════════════════════════════════════════

    /// Debounced config save timer fired. Persist config to disk.
    ConfigSaveTick,

    /// Config loaded from disk at startup.
    ConfigLoaded(AppConfig),

    /// Config save completed (or failed — log but don't crash).
    ConfigSaved(Result<(), String>),

    // ═══════════════════════════════════════════════════════════════
    // CONNECTION STATUS (future — Phase 7)
    // ═══════════════════════════════════════════════════════════════

    /// Connection status changed (for status bar display).
    ConnectionStatusChanged(ConnectionStatus),

    // ═══════════════════════════════════════════════════════════════
    // NO-OP / INTERNAL
    // ═══════════════════════════════════════════════════════════════

    /// A no-op message. Used when a Command produces no meaningful result.
    Noop,
}

/// Semantic keyboard actions (decoupled from physical keys).
#[derive(Debug, Clone)]
pub enum KeyAction {
    ZoomIn,
    ZoomOut,
    PanLeft,
    PanRight,
    JumpToLatest,
    JumpToOldest,
    QuickTimeframe(Timeframe),
    FocusSymbolSearch,
    LayoutPreset(u8),      // 1-4
    DeleteSelected,
    Escape,
    ToggleFrameOverlay,
}

/// Mouse button enum (iced provides this but we define for clarity).
#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}
```

### Message Flow Diagram

```
User Action              Message                          State Change
─────────────────────────────────────────────────────────────────────────
Mouse move on chart  →   ChartMouseMoved(id,x,y)      →  Update crosshair pos
                                                          Broadcast synced crosshair
Scroll wheel         →   ChartScrolled(id,dx,dy)      →  Zoom/pan camera
                                                          dirty.mark_camera()
                                                          Trigger Y auto-scale
Click on chart       →   ChartActivated(id)            →  Set active_chart
                         ChartMousePressed(id,btn,x,y) →  Start drag / select level
Type in search       →   ToolbarSearchChanged(text)    →  Filter suggestions
Press Enter          →   ToolbarSearchSubmitted         →  Dispatch ChartSymbolChanged
                     →   ChartSymbolChanged(id, sym)   →  Set LoadState::Loading
                                                          Return Command::perform(load)
Async load completes →   DataLoaded(id, buffer)        →  Store data, compute indicators
                                                          Set LoadState::Loaded
                                                          dirty.mark_data()
Timer tick           →   AnimationTick(instant)        →  Advance Y-axis lerp
                                                          Check if still animating
```

---

## 3. Update Function

### Entry Point

In iced 0.14, the application implements the `iced::Program` trait (or uses the `iced::application()` builder). The `update` function receives a `Message` and returns a `Task<Message>` (the iced 0.13+ rename of `Command`).

```rust
impl MidasApp {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // ── Chart interaction ─────────────────────────────────
            Message::ChartMouseMoved(id, x, y) => {
                self.handle_chart_mouse_moved(id, x, y)
            }
            Message::ChartMouseExited(id) => {
                self.handle_chart_mouse_exited(id)
            }
            Message::ChartMousePressed(id, button, x, y) => {
                self.handle_chart_mouse_pressed(id, button, x, y)
            }
            Message::ChartMouseReleased(id, button, x, y) => {
                self.handle_chart_mouse_released(id, button, x, y)
            }
            Message::ChartScrolled(id, dx, dy) => {
                self.handle_chart_scrolled(id, dx, dy)
            }
            Message::ChartDoubleClick(id, x, y) => {
                self.handle_chart_double_click(id, x, y)
            }

            // ── Chart state ───────────────────────────────────────
            Message::ChartActivated(id) => {
                self.active_chart = Some(id);
                Task::none()
            }
            Message::ChartSymbolChanged(id, symbol) => {
                self.handle_symbol_changed(id, symbol)
            }
            Message::ChartTimeframeChanged(id, tf) => {
                self.handle_timeframe_changed(id, tf)
            }
            Message::ChartToggleTimeLink(id) => {
                if let Some(chart) = self.charts.get_mut(&id) {
                    chart.time_linked = !chart.time_linked;
                }
                Task::none()
            }

            // ── Levels ────────────────────────────────────────────
            Message::LevelAdd(id, price) => {
                self.handle_level_add(id, price)
            }
            // ... other level messages follow same pattern ...

            // ── Layout ────────────────────────────────────────────
            Message::LayoutPresetSelected(preset) => {
                self.handle_layout_preset(preset)
            }
            Message::ChartAdd => {
                self.handle_chart_add()
            }
            Message::ChartRemove(id) => {
                self.handle_chart_remove(id)
            }

            // ── Toolbar ───────────────────────────────────────────
            Message::ToolbarSearchChanged(text) => {
                self.handle_search_changed(text)
            }
            Message::ToolbarSearchSubmitted => {
                self.handle_search_submitted()
            }
            Message::ToolbarSuggestionSelected(symbol) => {
                self.handle_suggestion_selected(symbol)
            }

            // ── Data ──────────────────────────────────────────────
            Message::DataLoaded(id, buffer) => {
                self.handle_data_loaded(id, buffer)
            }
            Message::DataLoadFailed(id, error) => {
                self.handle_data_load_failed(id, error)
            }
            Message::AvailableSymbolsLoaded(symbols) => {
                self.available_symbols = symbols;
                Task::none()
            }

            // ── Animation ─────────────────────────────────────────
            Message::AnimationTick(now) => {
                self.handle_animation_tick(now)
            }

            // ── Keyboard ──────────────────────────────────────────
            Message::KeyboardAction(action) => {
                self.handle_keyboard_action(action)
            }

            // ── Window ────────────────────────────────────────────
            Message::WindowResized(w, h) => {
                self.window_size = (w, h);
                // All chart viewports recalculate in view() based on layout
                Task::none()
            }
            Message::WindowScaleFactorChanged(factor) => {
                self.scale_factor = factor;
                // Mark all charts dirty so they re-render at new DPI
                for chart in self.charts.values_mut() {
                    chart.dirty.mark_camera();
                }
                Task::none()
            }
            Message::WindowCloseRequested => {
                self.save_config_sync();
                iced::exit()
            }

            // ── Config ────────────────────────────────────────────
            Message::ConfigSaveTick => {
                self.handle_config_save()
            }
            Message::ConfigLoaded(config) => {
                self.handle_config_loaded(config)
            }
            Message::ConfigSaved(_) => Task::none(),

            // ── Connection ────────────────────────────────────────
            Message::ConnectionStatusChanged(status) => {
                self.connection_status = status;
                Task::none()
            }

            // ── Sidebar ───────────────────────────────────────────
            Message::ToolbarToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                Task::none()
            }
            Message::WatchlistSymbolClicked(symbol) => {
                if let Some(id) = self.active_chart {
                    self.handle_symbol_changed(id, symbol)
                } else {
                    Task::none()
                }
            }
            // ... other watchlist messages ...

            Message::Noop => Task::none(),

            // Catch-all for messages not yet shown in detail above
            _ => Task::none(),
        }
    }
}
```

### Critical Handler Implementations

#### Symbol Change (triggers async data load)

```rust
impl MidasApp {
    fn handle_symbol_changed(&mut self, id: ChartId, symbol: String) -> Task<Message> {
        let chart = match self.charts.get_mut(&id) {
            Some(c) => c,
            None => return Task::none(),
        };

        // Update chart state
        chart.symbol = Some(symbol.clone());
        chart.load_state = LoadState::Loading;
        chart.data = None;
        chart.dirty.mark_data();

        // Mark config dirty for persistence
        self.config_dirty = true;

        // Dispatch async load
        let data_manager = Arc::clone(&self.data_manager);
        let timeframe = chart.timeframe;

        Task::perform(
            async move {
                // This runs on the tokio runtime.
                // Try binary file first, fall back to CSV.
                match data_manager.load_symbol(&symbol, timeframe).await {
                    Ok(buffer) => (id, Ok(Arc::new(buffer))),
                    Err(e) => (id, Err(e.to_string())),
                }
            },
            |(chart_id, result)| match result {
                Ok(buffer) => Message::DataLoaded(chart_id, buffer),
                Err(err) => Message::DataLoadFailed(chart_id, err),
            },
        )
    }
}
```

#### Data Loaded (completes async load)

```rust
impl MidasApp {
    fn handle_data_loaded(&mut self, id: ChartId, buffer: Arc<CandleBuffer>) -> Task<Message> {
        let chart = match self.charts.get_mut(&id) {
            Some(c) => c,
            None => return Task::none(),
        };

        chart.data = Some(Arc::clone(&buffer));
        chart.load_state = LoadState::Loaded;
        chart.dirty.mark_data();

        // Auto-fit camera to show all data
        if buffer.len() > 0 {
            let last_ts = buffer.timestamps[buffer.len() - 1] as f64;
            let visible_count = 200.min(buffer.len());
            let first_visible_ts = buffer.timestamps[buffer.len() - visible_count] as f64;

            chart.camera.time_start = first_visible_ts;
            chart.camera.time_end = last_ts
                + (last_ts - first_visible_ts) * 0.05; // 5% right padding

            // Compute Y range for visible data
            let range = (buffer.len() - visible_count)..buffer.len();
            let (low, high) = buffer.price_range(range);
            let padding = (high - low) as f64 * 0.05;
            chart.camera.price_low = low as f64 - padding;
            chart.camera.price_high = high as f64 + padding;

            chart.dirty.mark_camera();
        }

        // TODO(Phase 5): Initialize indicators for the new data
        // self.indicator_engine.initialize(id, &buffer);

        Task::none()
    }
}
```

#### Chart Scrolled (zoom/pan)

```rust
impl MidasApp {
    fn handle_chart_scrolled(&mut self, id: ChartId, dx: f32, dy: f32) -> Task<Message> {
        let chart = match self.charts.get_mut(&id) {
            Some(c) => c,
            None => return Task::none(),
        };

        // dy > 0 = scroll up = zoom in
        if dy.abs() > 0.01 {
            let zoom_factor = if dy > 0.0 { 0.9 } else { 1.1 };
            let center_x = chart.mouse_position
                .map(|(x, _)| x)
                .unwrap_or(chart.camera.viewport_width as f32 / 2.0);
            chart.camera.zoom(center_x, zoom_factor);
        }

        // dx for horizontal scroll (time pan)
        if dx.abs() > 0.01 {
            chart.camera.pan(dx * 3.0, 0.0); // Scale factor for scroll sensitivity
        }

        chart.dirty.mark_camera();
        self.trigger_y_autoscale(id);

        // If time-linked, propagate to other charts
        if chart.time_linked {
            self.propagate_time_axis(id);
        }

        // Start animation for Y-axis smooth scaling
        self.animating = true;

        Task::none()
    }
}
```

#### Animation Tick

```rust
impl MidasApp {
    fn handle_animation_tick(&mut self, now: time::Instant) -> Task<Message> {
        let mut any_animating = false;

        for chart in self.charts.values_mut() {
            if chart.y_animating {
                let t = 0.15; // Lerp factor per frame (~9 frames to converge)
                let dl = (chart.target_price_low - chart.camera.price_low) * t;
                let dh = (chart.target_price_high - chart.camera.price_high) * t;

                chart.camera.price_low += dl;
                chart.camera.price_high += dh;
                chart.dirty.mark_camera();

                // Check convergence (within 0.01% of target)
                let range = chart.camera.price_high - chart.camera.price_low;
                if dl.abs() < range * 0.0001 && dh.abs() < range * 0.0001 {
                    chart.camera.price_low = chart.target_price_low;
                    chart.camera.price_high = chart.target_price_high;
                    chart.y_animating = false;
                } else {
                    any_animating = true;
                }
            }
        }

        self.animating = any_animating;
        Task::none()
    }
}
```

### Which Messages Trigger Re-renders

iced re-renders the view after every `update()` call. The question is which updates cause the Shader widget to re-upload GPU data. This is controlled by generation counters in `DirtyFlags` (see canonical definition in chart-interaction-system.md):

| Counter | Incremented By | Checked By |
|---|---|---|
| `candles` | `dirty.mark_data()` — DataLoaded, RealtimeCandleUpdate | `DirtyTracker::needs_candle_rebuild()` — re-uploads instance buffers |
| `camera` | `dirty.mark_camera()` — ChartScrolled, AnimationTick, WindowResized | `DirtyTracker::needs_camera_update()` — re-uploads uniform buffer |
| `levels` | `dirty.mark_levels()` — LevelAdd, LevelMoved, LevelDelete | `DirtyTracker::needs_level_rebuild()` — re-uploads level geometry |
| `indicators` | `dirty.mark_data()` — DataLoaded, indicator add/remove | `DirtyTracker::needs_indicator_rebuild()` — re-uploads line geometry |
| `crosshair` | `dirty.mark_crosshair()` — MouseMoved | `DirtyTracker::needs_crosshair_update()` — re-uploads crosshair UBO |
| `grid` | `dirty.mark_camera()` — zoom changes grid density | `DirtyTracker::needs_grid_rebuild()` — re-uploads grid instances |
| `theme` | `dirty.mark_theme()` — theme change | `DirtyTracker::needs_theme_rebuild()` — re-uploads all colors |

**No clearing needed.** The `DirtyTracker` (owned by `ChartGpuResources` in the `ChartPipeline`) remembers the last-seen generation for each counter. In `Primitive::prepare()`, it compares current vs last-seen; if different, the corresponding GPU resource is re-uploaded and the tracker is updated. Since `prepare()` takes `&self` (immutable Primitive), and the tracker lives in `&mut Self::Pipeline`, this works without needing to mutate the application state.

---

## 4. View Function

### Widget Tree Structure

The view function builds an iced widget tree every frame. iced diffs the tree and only re-renders changed widgets. The structure mirrors a classic desktop application layout:

```
Window
└── Column (vertical stack, fills window)
    ├── Toolbar (fixed height, ~40px)
    │   └── Row
    │       ├── SymbolSearchInput
    │       ├── TimeframeButtons (Row of Buttons)
    │       ├── LayoutPresetButtons (Row of Buttons)
    │       ├── Spacer (fills remaining width)
    │       └── SidebarToggleButton
    ├── Row (fills remaining space)
    │   ├── Sidebar (fixed width ~200px, conditional)
    │   │   └── Scrollable Column
    │   │       └── WatchlistEntries...
    │   └── Workspace (fills remaining width)
    │       └── Dynamic chart grid (see below)
    └── StatusBar (fixed height, ~24px)
        └── Row
            ├── ConnectionStatusIndicator
            ├── Spacer
            └── ClockDisplay
```

### Complete View Function

```rust
impl MidasApp {
    pub fn view(&self) -> Element<Message> {
        let toolbar = self.view_toolbar();
        let content_area = self.view_content_area();
        let status_bar = self.view_status_bar();

        column![
            toolbar,
            content_area,
            status_bar,
        ]
        .into()
    }

    fn view_content_area(&self) -> Element<Message> {
        let workspace = self.view_workspace();

        if self.sidebar_visible {
            let sidebar = self.view_sidebar();
            row![
                sidebar,
                // Thin vertical divider
                container(vertical_rule(1))
                    .style(|_| container::Style {
                        border: Border::default(),
                        ..Default::default()
                    }),
                workspace,
            ]
            .into()
        } else {
            workspace.into()
        }
    }

    fn view_workspace(&self) -> Element<Message> {
        if self.layout.is_empty() {
            // Empty workspace — show welcome screen
            return self.view_empty_workspace();
        }

        // Recursively build the widget tree from the binary split tree.
        // See window-management-and-chart-layout.md Section 9 for the
        // full integration strategy (Strategy A: Row/Column/FillPortion).
        self.view_layout_node(self.layout.root.as_ref().unwrap())
    }

    /// Recursively render a LayoutNode from the binary split tree.
    /// - LayoutNode::Leaf  → look up PaneId in pane_chart_map → render chart or empty cell
    /// - LayoutNode::Split → row!/column! with FillPortion based on split ratio
    fn view_layout_node(&self, node: &LayoutNode) -> Element<Message> {
        match node {
            LayoutNode::Leaf(leaf) => {
                // The active tab's PaneId determines what to render.
                let pane_id = leaf.tabs[leaf.active_tab];
                self.view_pane_cell(pane_id)
            }
            LayoutNode::Split(split) => {
                let first = self.view_layout_node(&split.first);
                let second = self.view_layout_node(&split.second);

                // Convert ratio to FillPortion weights (multiply by 1000
                // for 0.1% granularity — matches window-management doc).
                let first_portion = (split.ratio * 1000.0) as u16;
                let second_portion = ((1.0 - split.ratio) * 1000.0) as u16;

                match split.axis {
                    Axis::Horizontal => {
                        row![
                            container(first)
                                .width(FillPortion(first_portion)),
                            // Resize handle (thin draggable bar)
                            resize_handle_vertical(split.id),
                            container(second)
                                .width(FillPortion(second_portion)),
                        ]
                        .height(Fill)
                        .into()
                    }
                    Axis::Vertical => {
                        column![
                            container(first)
                                .height(FillPortion(first_portion)),
                            // Resize handle (thin draggable bar)
                            resize_handle_horizontal(split.id),
                            container(second)
                                .height(FillPortion(second_portion)),
                        ]
                        .width(Fill)
                        .into()
                    }
                }
            }
        }
    }

    /// Render a single pane cell by PaneId.
    /// Looks up the PaneId → ChartId mapping. If a chart is assigned,
    /// render the Shader widget. If the pane is empty, render "Add Chart".
    fn view_pane_cell(&self, pane_id: PaneId) -> Element<Message> {
        match self.pane_chart_map.get(&pane_id) {
            Some(&chart_id) => match self.charts.get(&chart_id) {
                Some(chart) => self.view_chart_panel(chart),
                None => self.view_empty_cell(), // Stale ChartId — shouldn't happen
            },
            None => self.view_empty_cell(), // Unmapped pane — empty slot
        }
    }

    /// Render a chart panel: header bar + Shader widget.
    fn view_chart_panel(&self, chart: &ChartPanel) -> Element<Message> {
        let id = chart.id;
        let is_active = self.active_chart == Some(id);

        // ── Chart header (symbol, timeframe, link icon) ──
        let header = self.view_chart_header(chart, is_active);

        // ── Chart body (GPU-rendered content) ──
        let body: Element<Message> = match chart.load_state {
            LoadState::Empty => {
                // No symbol loaded — show prompt
                container(
                    text("Click toolbar or press Ctrl+F to load a symbol")
                        .size(14)
                        .color(self.theme.text_muted)
                )
                .center_x(Fill)
                .center_y(Fill)
                .style(|_| container::Style {
                    background: Some(self.theme.chart_background.into()),
                    ..Default::default()
                })
                .width(Fill)
                .height(Fill)
                .into()
            }
            LoadState::Loading => {
                // Show spinner / loading indicator
                container(
                    column![
                        text("Loading...").size(14).color(self.theme.text_muted),
                        // iced doesn't have a built-in spinner, so we use text.
                        // A real implementation would use a custom animated widget
                        // or a rotating SVG icon.
                    ]
                    .align_x(Center)
                )
                .center_x(Fill)
                .center_y(Fill)
                .style(|_| container::Style {
                    background: Some(self.theme.chart_background.into()),
                    ..Default::default()
                })
                .width(Fill)
                .height(Fill)
                .into()
            }
            LoadState::Error(ref msg) => {
                container(
                    text(format!("Error: {}", msg))
                        .size(14)
                        .color(self.theme.error_color)
                )
                .center_x(Fill)
                .center_y(Fill)
                .width(Fill)
                .height(Fill)
                .into()
            }
            LoadState::Loaded => {
                // THE CRITICAL INTEGRATION POINT:
                // iced's Shader widget wrapping our custom wgpu chart renderer.
                // view() builds ChartProgram with a fresh snapshot each frame.
                // Program::draw() reads from self.snapshot, calls
                // compute_chart_scene(), and wraps the result in ChartPrimitive.
                let snapshot = self.build_chart_snapshot(id, chart);
                shader(ChartProgram { chart_id: id, snapshot })
                    .width(Fill)
                    .height(Fill)
                    .into()
            }
        };

        // Wrap in a container with border highlight for active chart
        let border_color = if is_active {
            self.theme.active_chart_border
        } else {
            self.theme.chart_border
        };

        container(
            column![header, body]
        )
        .style(move |_| container::Style {
            border: Border {
                color: border_color,
                width: if is_active { 2.0 } else { 1.0 },
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .width(Fill)
        .height(Fill)
        .into()
    }

    // NOTE: view_split_tree() has been removed. Recursive layout rendering
    // is now handled by view_layout_node() above, which operates directly
    // on the LayoutNode tree from window-management-and-chart-layout.md.

    fn view_empty_workspace(&self) -> Element<Message> {
        container(
            column![
                text("Hand of Midas").size(28).color(self.theme.text_primary),
                vertical_space().height(16),
                text("Press Ctrl+F to search for a symbol, or choose a layout to begin.")
                    .size(14)
                    .color(self.theme.text_muted),
                vertical_space().height(24),
                button(text("+ Add Chart").size(14))
                    .on_press(Message::ChartAdd)
                    .padding([8, 16])
            ]
            .align_x(Center)
        )
        .center_x(Fill)
        .center_y(Fill)
        .width(Fill)
        .height(Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.1, 0.1, 0.12).into()),
            ..Default::default()
        })
        .into()
    }

    fn view_empty_cell(&self) -> Element<Message> {
        container(
            button(text("+").size(24))
                .on_press(Message::ChartAdd)
                .padding([12, 20])
        )
        .center_x(Fill)
        .center_y(Fill)
        .width(Fill)
        .height(Fill)
        .style(|_| container::Style {
            background: Some(Color::from_rgb(0.08, 0.08, 0.10).into()),
            border: Border {
                color: Color::from_rgb(0.2, 0.2, 0.25),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
    }
}
```

### Why Not iced::widget::pane_grid?

iced provides a `pane_grid` widget for split-pane layouts. It handles drag-to-resize dividers natively. We should strongly consider using it instead of manual row/column layout for the workspace. The trade-off:

**Pros of pane_grid:**
- Built-in divider drag handling
- Handles focus/active pane tracking
- Well-tested in iced ecosystem

**Cons of pane_grid:**
- Slightly less control over the exact divider styling
- API constrains you to its tree model (which matches our binary split tree)

**Recommendation**: Use `pane_grid` for the workspace if its API matches our needs. Fall back to the manual Row/Column/FillPortion approach (shown in `view_layout_node` above and in window-management-and-chart-layout.md Section 9) only if pane_grid introduces limitations. A pane_grid version would replace the recursive `view_layout_node` call:

```rust
fn view_workspace_pane_grid(&self) -> Element<Message> {
    PaneGrid::new(&self.pane_state, |id, pane, _is_maximized| {
        let chart_id = pane.chart_id;
        pane_grid::Content::new(
            match self.charts.get(&chart_id) {
                Some(chart) => self.view_chart_panel(chart),
                None => self.view_empty_cell(),
            }
        )
        .title_bar(pane_grid::TitleBar::new(
            self.view_chart_header_for_pane(chart_id)
        ))
    })
    .on_resize(10, Message::PaneResized)
    .on_click(Message::PaneClicked)
    .into()
}
```

---

## 5. Shader Widget (Chart-to-iced Bridge)

### Overview

This is the most architecturally critical section. iced's `shader` widget allows injecting custom wgpu rendering into the iced widget tree. Each chart panel uses a Shader widget that owns a `ChartRenderer` in its state and renders the chart's candles, volume, grid, indicators, crosshair, and levels using our custom wgpu pipelines.

### iced 0.14 Shader API (Pipeline Pattern)

In iced 0.14, the Shader widget uses the `shader::Program` trait with a new `Pipeline`
associated type on `Primitive`. This replaces the old `Storage`-based approach from 0.13.

```rust
// From iced 0.14 source (simplified):
pub trait Program<Message> {
    /// Per-widget CPU state. Created once, updated on each draw().
    type State: Default;

    /// Primitives prepared on the CPU for rendering.
    type Primitive: Primitive;

    /// Called each frame in the view phase to build the Primitive.
    /// Runs on the main thread. Should be fast — do CPU work here,
    /// prepare GPU upload data, but don't issue GPU commands.
    fn draw(
        &self,
        state: &Self::State,
        cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive;

    /// Called when the widget receives a mouse/keyboard event.
    /// Returns (event::Status, Option<Message>).
    fn update(
        &self,
        state: &mut Self::State,
        event: Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> (event::Status, Option<Message>);

    /// Mouse interaction style (cursor icon).
    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction;
}

/// iced 0.14 Primitive trait — uses associated Pipeline type instead of Storage.
pub trait Primitive: Send + Debug + 'static {
    /// The GPU pipeline state. Created once by iced via Pipeline::new() and
    /// shared across ALL widget instances of this Primitive type.
    type Pipeline: Pipeline;

    /// Called to prepare GPU resources (upload buffers, etc).
    /// Receives a mutable reference to the shared Pipeline.
    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    );

    /// Called to issue GPU draw commands via a live RenderPass.
    /// Return true if draw calls were issued.
    fn draw(
        &self,
        pipeline: &Self::Pipeline,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> bool;

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    );
}

/// iced 0.14 Pipeline trait — constructed once with GPU device access.
pub trait Pipeline {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self;
}
```

The split is important: `Program::draw()` is called in the `view` phase to build the Primitive (CPU data extraction), and `Primitive::prepare()`, `Primitive::draw()`, and `Primitive::render()` are called later by iced's rendering backend with access to wgpu resources.

**Key change from iced 0.13**: The `Storage` type-keyed map is gone. Instead, each
`Primitive` declares an associated `type Pipeline` that iced constructs once via
`Pipeline::new(device, queue, format)`. The Pipeline instance is shared across ALL
widget instances of the same Primitive type. This means our `SharedPipelines` become
part of the Pipeline, and per-chart state is stored in a `HashMap<ChartId, ChartGpuResources>`
inside the Pipeline struct.

### Our Implementation

```rust
use iced::widget::shader;
use iced::widget::shader::{self, Viewport};
use iced::{mouse, event, Rectangle, Color};
use std::collections::HashMap;
use wgpu;

/// The shader::Program implementation for a chart panel.
/// ChartProgram no longer borrows &MidasApp — eliminating the worst
/// coupling point in the old design. Instead, view() builds a
/// ChartProgram with a fresh ChartRenderSnapshot each frame.
/// Program::draw() reads from self.snapshot, builds a ChartInput,
/// calls the pure function compute_chart_scene(), and wraps the
/// result in ChartPrimitive.
pub struct ChartProgram {
    pub chart_id: ChartId,
    pub snapshot: ChartRenderSnapshot,  // Built fresh in view() each frame
}

/// Per-widget CPU state. Created once by iced per Shader widget instance.
/// Note: GPU state (pipelines, buffers) lives in ChartPipeline, NOT here.
/// This struct holds per-widget CPU-side state for viewport detection,
/// hover state, and similar concerns. The ChartRenderSnapshot is NOT
/// stored here — it lives on ChartProgram, which is built fresh in
/// view() each frame.
#[derive(Default)]
pub struct ChartShaderState {
    /// Tracks last known viewport size for resize detection.
    last_viewport: Option<(u32, u32)>,

    /// Hover state, interaction tracking, and other per-widget state
    /// that persists across frames.
}

/// Snapshot of all chart data needed by compute_chart_scene().
/// Built fresh in view() and placed directly on ChartProgram each frame.
/// This is the mechanism by which data flows from MidasApp to the
/// Shader widget WITHOUT the widget borrowing &MidasApp.
#[derive(Clone, Debug)]
pub struct ChartRenderSnapshot {
    pub data: Arc<CandleBuffer>,
    pub camera: Camera2D,
    pub viewport: Viewport,
    pub theme: ChartTheme,
    pub crosshair: Option<CrosshairState>,
    pub levels: Vec<HorizontalLevel>,
    pub indicators: Vec<IndicatorOutput>,
    pub dirty: DirtyFlags,
    /// Current cursor icon derived from InteractionState.
    /// Computed by the application layer so mouse_interaction()
    /// does not need to access &MidasApp.
    pub interaction_cursor: InteractionCursor,
}

/// Cursor icon hint derived from the chart's InteractionState.
/// Computed in view() when building the ChartRenderSnapshot.
#[derive(Clone, Debug, Default)]
pub enum InteractionCursor {
    #[default]
    Crosshair,
    Grabbing,
    ResizeVertical,
    Default,
}

/// The intermediate data produced by Program::draw() and consumed
/// by Primitive::prepare() and Primitive::draw().
///
/// ChartPrimitive wraps a ChartScene — the framework-agnostic intermediate
/// representation produced by compute_chart_scene(). It does NOT carry
/// loose Vec<CandleInstance> fields or access &MidasApp.
/// See gpu-rendering-architecture.md Section 1.6 for ChartScene definition.
#[derive(Debug)]
pub struct ChartPrimitive {
    pub chart_id: ChartId,
    pub scene: ChartScene,
}

impl shader::Program<Message> for ChartProgram {
    type State = ChartShaderState;
    type Primitive = ChartPrimitive;

    /// Called each frame in the view phase to build the ChartPrimitive.
    /// This is `Program::draw()` per iced 0.14's Shader API — not to be
    /// confused with `Primitive::draw()` which issues GPU draw commands.
    ///
    /// KEY CHANGE: This method no longer accesses &MidasApp. Instead, it
    /// reads from self.snapshot (built fresh in view() each frame).
    /// It builds a ChartInput from the snapshot, calls the pure function
    /// compute_chart_scene(), and wraps the result in ChartPrimitive.
    fn draw(
        &self,
        _state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        // Read the chart snapshot directly from ChartProgram.
        // view() builds ChartProgram with a fresh snapshot each frame.
        let snapshot = &self.snapshot;

        // Build the clean input contract — explicit, testable, no framework deps.
        let input = ChartInput {
            data: snapshot.data.as_ref(),
            camera: &snapshot.camera,
            viewport: &snapshot.viewport,
            theme: &snapshot.theme,
            crosshair: snapshot.crosshair.as_ref(),
            levels: &snapshot.levels,
            indicators: &snapshot.indicators,
            dirty: &snapshot.dirty,
        };

        // Call the pure function — all chart logic lives here.
        // See chart-interaction-system.md Section 1b for the full contract.
        let scene = midas_chart::compute_chart_scene(&input);

        ChartPrimitive {
            chart_id: self.chart_id,
            scene,
        }
    }

    fn update(
        &self,
        _state: &mut Self::State,
        event: shader::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        // Convert iced events to our Messages.
        // The Shader widget receives raw mouse/keyboard events.
        let id = self.chart_id;

        match event {
            shader::Event::Mouse(mouse_event) => {
                match mouse_event {
                    mouse::Event::CursorMoved { position } => {
                        // Convert to chart-local coordinates
                        if bounds.contains(position) {
                            let local_x = position.x - bounds.x;
                            let local_y = position.y - bounds.y;
                            (
                                event::Status::Captured,
                                Some(Message::ChartMouseMoved(id, local_x, local_y)),
                            )
                        } else {
                            (
                                event::Status::Ignored,
                                Some(Message::ChartMouseExited(id)),
                            )
                        }
                    }
                    mouse::Event::ButtonPressed(btn) => {
                        if let Some(pos) = cursor.position_in(bounds) {
                            let button = match btn {
                                iced::mouse::Button::Left => MouseButton::Left,
                                iced::mouse::Button::Right => MouseButton::Right,
                                iced::mouse::Button::Middle => MouseButton::Middle,
                                _ => return (event::Status::Ignored, None),
                            };
                            (
                                event::Status::Captured,
                                Some(Message::ChartMousePressed(id, button, pos.x, pos.y)),
                            )
                        } else {
                            (event::Status::Ignored, None)
                        }
                    }
                    mouse::Event::ButtonReleased(btn) => {
                        if let Some(pos) = cursor.position_in(bounds) {
                            let button = match btn {
                                iced::mouse::Button::Left => MouseButton::Left,
                                iced::mouse::Button::Right => MouseButton::Right,
                                iced::mouse::Button::Middle => MouseButton::Middle,
                                _ => return (event::Status::Ignored, None),
                            };
                            (
                                event::Status::Captured,
                                Some(Message::ChartMouseReleased(id, button, pos.x, pos.y)),
                            )
                        } else {
                            (event::Status::Ignored, None)
                        }
                    }
                    mouse::Event::WheelScrolled { delta } => {
                        if cursor.is_over(bounds) {
                            let (dx, dy) = match delta {
                                mouse::ScrollDelta::Lines { x, y } => (x, y),
                                mouse::ScrollDelta::Pixels { x, y } => (x, y),
                            };
                            (
                                event::Status::Captured,
                                Some(Message::ChartScrolled(id, dx, dy)),
                            )
                        } else {
                            (event::Status::Ignored, None)
                        }
                    }
                    _ => (event::Status::Ignored, None),
                }
            }
            shader::Event::Keyboard(_key_event) => {
                // Keyboard events are handled globally via subscriptions,
                // not per-chart. Pass through.
                (event::Status::Ignored, None)
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            // Read interaction state from self.snapshot (built fresh in view()).
            match self.snapshot.interaction_cursor {
                InteractionCursor::Grabbing => mouse::Interaction::Grabbing,
                InteractionCursor::ResizeVertical => mouse::Interaction::ResizingVertically,
                InteractionCursor::Crosshair | InteractionCursor::Default =>
                    mouse::Interaction::Crosshair,
            }
        } else {
            mouse::Interaction::default()
        }
    }
}
```

### Primitive GPU Integration

```rust
impl shader::Primitive for ChartPrimitive {
    /// The Pipeline associated type — created once by iced, shared across
    /// ALL ChartPrimitive instances (all chart widgets).
    type Pipeline = ChartPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        // The pipeline is shared across all charts. Per-chart state is
        // stored in pipeline.charts HashMap, keyed by ChartId.
        // On first call for a chart, create its ChartGpuResources.

        let chart_gpu = pipeline.charts
            .entry(self.chart_id)
            .or_insert_with(|| ChartGpuResources::new(
                device,
                &pipeline.shared,
                self.chart_id,
            ));

        // All data now comes from self.scene (a ChartScene produced by
        // compute_chart_scene()). Compare the scene's generation counters
        // against the tracker's last-seen generations. Only upload what
        // actually changed.

        let scene = &self.scene;
        let gens = &scene.generations;
        let tracker = &mut chart_gpu.dirty_tracker;

        // Upload uniform buffer (projection matrix) if camera changed
        if tracker.needs_camera_update_gen(gens.camera) {
            chart_gpu.update_projection(queue, &scene.projection);
        }

        // Upload candle instance buffer if candle data changed
        if tracker.needs_candle_rebuild_gen(gens.candles) {
            if let Some(ref candles) = scene.candles {
                chart_gpu.candle_instances.write(device, queue, candles);
                chart_gpu.candle_count = scene.candle_count as u32;
            }
            if let Some(ref volumes) = scene.volumes {
                chart_gpu.volume_instances.write(device, queue, volumes);
                chart_gpu.volume_count = scene.volume_count as u32;
            }
        }

        // Grid lines and axis labels update whenever camera changes
        if tracker.needs_camera_update_gen(gens.camera) {
            chart_gpu.grid_instances.write(device, queue, &scene.grid_lines);
            chart_gpu.grid_count = scene.grid_lines.len() as u32;
            // Axis labels (x + y)
            let all_labels: Vec<_> = scene.x_labels.iter()
                .chain(scene.y_labels.iter())
                .collect();
            chart_gpu.text_instances.write_labels(device, queue, &all_labels);
            chart_gpu.text_glyph_count = all_labels.len() as u32;
        }

        // Horizontal levels
        if tracker.needs_level_rebuild_gen(gens.levels) {
            chart_gpu.hline_instances.write(device, queue, &scene.levels);
            chart_gpu.hline_count = scene.levels.len() as u32;
        }

        // Crosshair (updated when crosshair generation changes)
        if tracker.needs_crosshair_update_gen(gens.crosshair) {
            if let Some(ref crosshair) = scene.crosshair {
                chart_gpu.crosshair_ubo.write(device, queue, crosshair);
            }
        }

        // Acknowledge all current generations — next frame will only
        // re-upload if a counter has been incremented again.
        tracker.acknowledge_generations(gens);
    }

    fn draw(
        &self,
        pipeline: &Self::Pipeline,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> bool {
        let chart_gpu = match pipeline.charts.get(&self.chart_id) {
            Some(g) => g,
            None => return false, // Not yet initialized
        };

        let shared = &pipeline.shared;

        // Set scissor and viewport are handled by iced before calling draw().

        // ── Draw order (back to front) ───────────────────────────
        // All rendering data comes from self.scene (ChartScene).
        // The GPU buffers were populated in prepare() from scene data.

        // 1. Background (clear our region)
        shared.rect_pipeline.draw_background(render_pass, &self.scene.viewport.background_color);

        // 2. Grid lines (very faint, behind everything)
        shared.grid_pipeline.draw(render_pass, chart_gpu);

        // 3. Volume bars (semi-transparent, behind candles)
        shared.volume_pipeline.draw(render_pass, chart_gpu);

        // 4. Horizontal levels
        shared.hline_pipeline.draw(render_pass, chart_gpu);

        // 5. Candle wicks (draw_mode=0 via bind group swap)
        shared.candle_pipeline.draw_wicks(render_pass, &shared.wick_params_bind_group, chart_gpu);

        // 6. Candle bodies (draw_mode=1 via bind group swap)
        shared.candle_pipeline.draw_bodies(render_pass, &shared.body_params_bind_group, chart_gpu);

        // 7. Axis labels (MSDF text pipeline)
        shared.text_pipeline.draw(render_pass, chart_gpu);

        // 8. Crosshair (on top of everything)
        if self.scene.crosshair.is_some() {
            shared.crosshair_pipeline.draw(render_pass, chart_gpu);
        }

        true // draw calls were issued
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        // In iced 0.14, draw() receives the RenderPass directly, so
        // render() is typically not needed for our use case. The draw()
        // method above handles all rendering. This method exists for
        // cases that need the CommandEncoder directly (e.g., compute
        // passes or texture copies), which we don't currently need.
    }
}

/// The iced 0.14 Pipeline type for chart rendering.
/// Created once by iced via Pipeline::new(). Shared across ALL chart widgets.
/// Contains both shared render pipelines and per-chart GPU resources.
pub struct ChartPipeline {
    /// Shared GPU resources (render pipelines, unit quad VBO, font atlas,
    /// draw params uniform buffer). Created once.
    shared: SharedPipelines,

    /// Per-chart GPU resources, keyed by ChartId.
    /// Created lazily on first prepare() for each chart.
    charts: HashMap<ChartId, ChartGpuResources>,
}

impl shader::Pipeline for ChartPipeline {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            shared: SharedPipelines::new(device, queue, format),
            charts: HashMap::new(),
        }
    }
}

/// Per-chart GPU resources. Stored inside ChartPipeline::charts.
pub struct ChartGpuResources {
    pub chart_id: ChartId,

    // Uniform buffers
    pub camera_ubo: wgpu::Buffer,
    pub camera_bind_group: wgpu::BindGroup,

    // Instance buffers (resizable via GrowableBuffer)
    pub candle_instances: GrowableBuffer,
    pub volume_instances: GrowableBuffer,
    pub grid_instances: GrowableBuffer,
    pub indicator_instances: GrowableBuffer,
    pub hline_instances: GrowableBuffer,
    pub crosshair_instances: GrowableBuffer,

    // Instance counts (set each frame)
    pub candle_count: u32,
    pub volume_count: u32,
    pub grid_count: u32,
    pub indicator_count: u32,
    pub hline_count: u32,

    /// Tracks which generation counters this GPU state has already
    /// processed. Compared against ChartScene::generations (via
    /// ChartPrimitive::scene) in Primitive::prepare() to skip
    /// unchanged uploads.
    pub dirty_tracker: DirtyTracker,
}

impl ChartGpuResources {
    pub fn new(
        device: &wgpu::Device,
        shared: &SharedPipelines,
        chart_id: ChartId,
    ) -> Self {
        // Create per-chart uniform buffers and initial instance buffers.
        // Render pipelines are NOT created here — they are shared via
        // SharedPipelines in the Pipeline struct.
        todo!("Per-chart resource creation — see Phase 1 implementation")
    }

    pub fn update_uniforms(&self, queue: &wgpu::Queue, projection: &glam::Mat4) {
        queue.write_buffer(
            &self.camera_ubo,
            0,
            bytemuck::bytes_of(projection),
        );
    }
}
```

### RESOLVED: Dirty Flag Reset Problem

**Problem (now solved):** `Primitive::prepare()` takes `&self` (immutable), so boolean dirty flags cannot be cleared by the renderer.

**Solution: Generation counters.** The `DirtyFlags` struct uses `u64` generation counters instead of booleans (see canonical definition in chart-interaction-system.md). Writers (in `update()`) increment counters. The `DirtyTracker` (owned by `ChartGpuResources` inside the `Pipeline`) remembers last-seen counter values. In `Primitive::prepare()`, the tracker compares against the `ChartScene`'s `SceneGenerations` and calls `acknowledge_generations()` to record the new generations. No flag clearing is needed, and `&self` immutability on the Primitive is not a problem because the tracker lives in `&mut Self::Pipeline`.

```rust
// In ChartState (owned by MidasApp):
pub dirty: DirtyFlags,    // generation counters, incremented by update()

// In ChartGpuResources (stored in ChartPipeline::charts HashMap):
dirty_tracker: DirtyTracker,  // last-seen generation snapshot

// In Primitive::prepare():
let chart_gpu = pipeline.charts.entry(self.chart_id).or_insert_with(|| ...);
let gens = &self.scene.generations;   // from ChartScene
let tracker = &mut chart_gpu.dirty_tracker;
if tracker.needs_candle_rebuild_gen(gens.candles) {
    if let Some(ref candles) = self.scene.candles {
        chart_gpu.candle_instances.write(device, queue, candles);
    }
}
// ... other checks ...
tracker.acknowledge_generations(gens);  // records current generations
```

This is the canonical approach. There is no "post_view_cleanup" pattern, no boolean clearing, and no risk of missed or double updates.

### RESOLVED: Open Question #1 — Pipeline Sharing

**Question**: Are `SharedPipelines` (render pipelines, unit quad VBO, font atlas)
created once and shared, or duplicated per widget?

**Answer (RESOLVED)**: In iced 0.14, the `Pipeline` associated type is created exactly
once by iced and shared across ALL widget instances of the same `Primitive` type.
`ChartPipeline` (our `impl shader::Pipeline`) contains `SharedPipelines` (created once
in `Pipeline::new()`) plus a `HashMap<ChartId, ChartGpuResources>` for per-chart state.
This is exactly the architecture we need: zero duplication of render pipelines, one font
atlas, one unit quad VBO, and per-chart instance buffers created lazily on first use.

---

## 6. Subscription System

### Overview

iced subscriptions are long-lived event streams that produce Messages. They are declared in the `subscription()` method and iced manages their lifecycle (starts them when they appear, stops them when they don't).

```rust
impl MidasApp {
    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = Vec::new();

        // ── 1. Animation tick (60fps when animating) ─────────────
        if self.animating || self.has_crosshair_visible() {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(Message::AnimationTick)
            );
        }

        // ── 2. Keyboard events (global) ──────────────────────────
        subscriptions.push(
            keyboard::on_key_press(|key, modifiers| {
                Self::map_key_to_action(key, modifiers)
                    .map(Message::KeyboardAction)
            })
        );

        // ── 3. Window events ─────────────────────────────────────
        subscriptions.push(
            iced::event::listen_with(|event, _status, _id| {
                match event {
                    iced::Event::Window(window::Event::Resized(size)) => {
                        Some(Message::WindowResized(
                            size.width as u32,
                            size.height as u32,
                        ))
                    }
                    iced::Event::Window(window::Event::CloseRequested) => {
                        Some(Message::WindowCloseRequested)
                    }
                    _ => None,
                }
            })
        );

        // ── 4. Config save debounce timer ────────────────────────
        if self.config_dirty {
            subscriptions.push(
                iced::time::every(std::time::Duration::from_secs(1))
                    .map(|_| Message::ConfigSaveTick)
            );
        }

        // ── 5. (Future) Real-time data stream ────────────────────
        // When we have WebSocket connections in Phase 7, each active
        // subscription becomes an iced Subscription:
        //
        // for symbol in self.active_subscriptions() {
        //     subscriptions.push(
        //         Subscription::run_with_id(
        //             format!("feed-{}", symbol),
        //             realtime_feed_stream(symbol.clone())
        //         )
        //         .map(Message::from_feed_event)
        //     );
        // }

        Subscription::batch(subscriptions)
    }
}
```

### Animation Tick Details

The animation subscription is conditional: it only runs when at least one chart has an active animation (Y-axis scaling, future: zoom animation, data streaming). When no animation is active, the subscription is absent and iced does not poll at 60fps, saving CPU/GPU.

```rust
impl MidasApp {
    fn has_crosshair_visible(&self) -> bool {
        self.charts.values().any(|c| c.crosshair_visible)
    }

    fn needs_animation(&self) -> bool {
        self.animating
            || self.has_crosshair_visible()
            || self.connection_status_is_streaming()
    }
}
```

When the user is hovering over any chart, the crosshair needs to render, which requires continuous ticks. When no mouse is over any chart and no animation is in progress, ticks stop entirely.

### Keyboard Shortcut Mapping

```rust
impl MidasApp {
    fn map_key_to_action(
        key: keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Option<KeyAction> {
        use keyboard::key::Named;

        match key {
            // Zoom
            keyboard::Key::Character(ref c) if c.as_str() == "=" || c.as_str() == "+" => {
                Some(KeyAction::ZoomIn)
            }
            keyboard::Key::Character(ref c) if c.as_str() == "-" => {
                Some(KeyAction::ZoomOut)
            }

            // Pan
            keyboard::Key::Named(Named::ArrowLeft) => Some(KeyAction::PanLeft),
            keyboard::Key::Named(Named::ArrowRight) => Some(KeyAction::PanRight),

            // Jump
            keyboard::Key::Named(Named::Home) => Some(KeyAction::JumpToLatest),
            keyboard::Key::Named(Named::End) => Some(KeyAction::JumpToOldest),

            // Quick timeframe (number keys without modifiers)
            keyboard::Key::Character(ref c) if !modifiers.command() => {
                match c.as_str() {
                    "1" => Some(KeyAction::QuickTimeframe(Timeframe::M1)),
                    "2" => Some(KeyAction::QuickTimeframe(Timeframe::M5)),
                    "3" => Some(KeyAction::QuickTimeframe(Timeframe::M15)),
                    "4" => Some(KeyAction::QuickTimeframe(Timeframe::H1)),
                    "5" => Some(KeyAction::QuickTimeframe(Timeframe::H4)),
                    "6" => Some(KeyAction::QuickTimeframe(Timeframe::D1)),
                    "7" => Some(KeyAction::QuickTimeframe(Timeframe::W1)),
                    _ => None,
                }
            }

            // Ctrl+F: focus symbol search
            keyboard::Key::Character(ref c)
                if c.as_str() == "f" && modifiers.command() =>
            {
                Some(KeyAction::FocusSymbolSearch)
            }

            // Ctrl+1..4: layout presets
            keyboard::Key::Character(ref c) if modifiers.command() => {
                match c.as_str() {
                    "1" => Some(KeyAction::LayoutPreset(1)),
                    "2" => Some(KeyAction::LayoutPreset(2)),
                    "3" => Some(KeyAction::LayoutPreset(3)),
                    "4" => Some(KeyAction::LayoutPreset(4)),
                    _ => None,
                }
            }

            // Delete: remove selected level
            keyboard::Key::Named(Named::Delete) => Some(KeyAction::DeleteSelected),

            // Escape: deselect / cancel
            keyboard::Key::Named(Named::Escape) => Some(KeyAction::Escape),

            // F11: toggle frame time overlay
            keyboard::Key::Named(Named::F11) => Some(KeyAction::ToggleFrameOverlay),

            _ => None,
        }
    }
}
```

### Future WebSocket Subscription Pattern

For Phase 7, each real-time feed connection will be an iced `Subscription::run()` that wraps a tokio-tungstenite stream:

```rust
fn realtime_feed_subscription(symbol: String, api_key: String) -> Subscription<Message> {
    Subscription::run_with_id(
        format!("feed-{}", symbol),
        stream::channel(100, move |mut sender| async move {
            loop {
                // Connect to WebSocket
                let ws = connect_polygon(&api_key).await;
                // Subscribe to symbol
                ws.subscribe(&symbol).await;

                // Process messages until disconnect
                while let Some(msg) = ws.next().await {
                    match msg {
                        FeedEvent::Trade(trade) => {
                            let _ = sender.send(
                                Message::RealtimeCandleUpdate(/* ... */)
                            ).await;
                        }
                        FeedEvent::Aggregate(agg) => {
                            let _ = sender.send(
                                Message::RealtimeCandleClosed(/* ... */)
                            ).await;
                        }
                        FeedEvent::Disconnected => break,
                    }
                }

                // Reconnect with backoff
                sender.send(Message::ConnectionStatusChanged(
                    ConnectionStatus::Reconnecting { attempt: 1 }
                )).await.ok();
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
    )
}
```

---

## 7. Async Data Loading

### Flow: Symbol Search to Rendered Chart

```
User types "AAPL" → presses Enter
    │
    ▼
Message::ToolbarSearchSubmitted
    │
    ▼ (in update)
MidasApp reads toolbar.search_text = "AAPL"
    │
    ▼ (dispatches)
Message::ChartSymbolChanged(active_chart_id, "AAPL")
    │
    ▼ (in update)
chart.load_state = LoadState::Loading
chart.symbol = Some("AAPL")
    │
    ▼ (returns)
Task::perform(async { data_manager.load_symbol("AAPL", D1) })
    │
    ▼ (iced executes on tokio runtime)
DataManager::load_symbol:
    1. Check in-memory cache → hit? return Arc clone
    2. Check binary file: data/candles/AAPL/1d.candles → exists? mmap + parse
    3. Check CSV file: data/csv/AAPL.csv → exists? parse + convert to binary
    4. None found → return Err("No data for AAPL")
    │
    ▼ (async completes)
Message::DataLoaded(chart_id, Arc<CandleBuffer>)
    │
    ▼ (in update)
chart.data = Some(buffer)
chart.load_state = LoadState::Loaded
chart.camera = auto_fit_to_data(buffer)
indicator_engine.initialize(chart_id, buffer)  // Phase 5 — stub in v1
chart.dirty.mark_data()
chart.dirty.mark_camera()
    │
    ▼ (next view)
view() builds ChartProgram with snapshot from chart panel state,
  ChartProgram::draw() reads self.snapshot, builds ChartInput,
  calls compute_chart_scene() → ChartScene,
  wraps in ChartPrimitive { chart_id, scene }
    │
    ▼ (Primitive::prepare)
GPU buffers uploaded from ChartScene data (candle/volume/grid/labels)
    │
    ▼ (Primitive::draw)
Chart visible on screen
```

### Task::perform Pattern (iced 0.14)

```rust
fn handle_symbol_changed(&mut self, id: ChartId, symbol: String) -> Task<Message> {
    // ... set loading state (shown above) ...

    let dm = Arc::clone(&self.data_manager);
    let tf = self.charts.get(&id)
        .map(|c| c.timeframe)
        .unwrap_or(Timeframe::D1);

    // Task::perform takes an async future and a mapping function.
    // iced runs the future on its built-in tokio runtime.
    Task::perform(
        async move {
            dm.load_symbol(&symbol, tf).await
        },
        move |result| match result {
            Ok(buffer) => Message::DataLoaded(id, Arc::new(buffer)),
            Err(e) => Message::DataLoadFailed(id, e.to_string()),
        },
    )
}
```

### Loading State UI

When `LoadState::Loading`, the chart panel shows a loading indicator instead of the Shader widget. This means no GPU resources are allocated for a chart that hasn't loaded data yet. The transition is:

```
Empty → (symbol entered) → Loading → (data arrives) → Loaded
                                    → (error)        → Error
```

The Error state displays the error message and allows the user to retry. A retry simply re-dispatches `ChartSymbolChanged`.

### Batch Loading at Startup

When restoring a saved layout with multiple charts, we issue parallel loads:

```rust
fn handle_config_loaded(&mut self, config: AppConfig) -> Task<Message> {
    // Restore layout
    self.layout = config.layout;

    // Create chart panels from config
    let mut tasks = Vec::new();
    for chart_config in &config.charts {
        let id = self.create_chart_panel(chart_config);
        if let Some(ref symbol) = chart_config.symbol {
            tasks.push(self.handle_symbol_changed(id, symbol.clone()));
        }
    }

    // Execute all loads concurrently
    Task::batch(tasks)
}
```

`Task::batch` runs all tasks concurrently on the tokio runtime. Each `DataLoaded` message arrives independently and updates its respective chart.

---

## 8. Toolbar Design

### Widget Structure

```rust
impl MidasApp {
    fn view_toolbar(&self) -> Element<Message> {
        let search_input = text_input("Symbol...", &self.toolbar.search_text)
            .on_input(Message::ToolbarSearchChanged)
            .on_submit(Message::ToolbarSearchSubmitted)
            .width(140)
            .padding([4, 8])
            .size(14);

        // Autocomplete dropdown (shown when search_focused and suggestions exist)
        let search_with_dropdown = if self.toolbar.search_focused
            && !self.toolbar.search_suggestions.is_empty()
        {
            // Overlay the dropdown below the search input.
            // iced does not have a built-in dropdown/overlay widget for
            // arbitrary content. Options:
            //   1. Use a column below the input (shifts layout — not ideal)
            //   2. Use iced's overlay mechanism (floating layer)
            //   3. Use a pick_list or combo_box widget
            //
            // Recommended: iced's combo_box widget, which provides
            // text input + filterable dropdown natively.
            combo_box(
                &self.toolbar.search_combo_state,
                "Symbol...",
                Some(&self.toolbar.search_text),
                Message::ToolbarSuggestionSelected,
            )
            .on_input(Message::ToolbarSearchChanged)
            .width(160)
            .padding([4, 8])
            .size(14)
            .into()
        } else {
            search_input.into()
        };

        // Timeframe buttons
        let timeframes = [
            (Timeframe::M1,  "1m"),
            (Timeframe::M5,  "5m"),
            (Timeframe::M15, "15m"),
            (Timeframe::H1,  "1H"),
            (Timeframe::H4,  "4H"),
            (Timeframe::D1,  "D"),
            (Timeframe::W1,  "W"),
        ];

        let active_tf = self.active_chart
            .and_then(|id| self.charts.get(&id))
            .map(|c| c.timeframe);

        let tf_buttons: Element<Message> = row(
            timeframes.iter().map(|(tf, label)| {
                let is_active = active_tf == Some(*tf);
                let tf_val = *tf;
                button(text(*label).size(12))
                    .on_press(Message::ToolbarTimeframeClicked(tf_val))
                    .padding([4, 8])
                    .style(move |theme, status| {
                        if is_active {
                            button::primary(theme, status) // Highlighted
                        } else {
                            button::secondary(theme, status) // Normal
                        }
                    })
                    .into()
            })
        )
        .spacing(2)
        .into();

        // Layout preset buttons — each sends a LayoutPreset enum variant.
        // The update handler calls the corresponding factory method on
        // WorkspaceLayout (e.g., preset_2x2()) to build the binary split tree.
        let layout_buttons: Element<Message> = row![
            button(text("1").size(12))
                .on_press(Message::LayoutPresetSelected(LayoutPreset::Single))
                .padding([4, 6]),
            button(text("2H").size(12))
                .on_press(Message::LayoutPresetSelected(LayoutPreset::SplitH))
                .padding([4, 6]),
            button(text("2V").size(12))
                .on_press(Message::LayoutPresetSelected(LayoutPreset::SplitV))
                .padding([4, 6]),
            button(text("4").size(12))
                .on_press(Message::LayoutPresetSelected(LayoutPreset::Grid2x2))
                .padding([4, 6]),
            button(text("6").size(12))
                .on_press(Message::LayoutPresetSelected(LayoutPreset::Grid3x2))
                .padding([4, 6]),
            button(text("8").size(12))
                .on_press(Message::LayoutPresetSelected(LayoutPreset::Grid4x2))
                .padding([4, 6]),
        ]
        .spacing(2)
        .into();

        // Sidebar toggle
        let sidebar_toggle = button(
            text(if self.sidebar_visible { "<<" } else { ">>" }).size(12)
        )
        .on_press(Message::ToolbarToggleSidebar)
        .padding([4, 8]);

        // Active symbol display
        let active_symbol_display = self.active_chart
            .and_then(|id| self.charts.get(&id))
            .and_then(|c| c.symbol.as_deref())
            .unwrap_or("---");

        // Assemble toolbar
        container(
            row![
                search_with_dropdown,
                horizontal_space().width(8),
                text(active_symbol_display).size(14).color(self.theme.text_primary),
                horizontal_space().width(16),
                tf_buttons,
                horizontal_space().width(16),
                vertical_rule(1),
                horizontal_space().width(16),
                layout_buttons,
                horizontal_space().width(Fill), // Push remaining to the right
                sidebar_toggle,
            ]
            .align_y(Center)
            .padding([4, 8])
        )
        .style(|_| container::Style {
            background: Some(self.theme.toolbar_background.into()),
            border: Border {
                color: self.theme.toolbar_border,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .width(Fill)
        .height(40)
        .into()
    }
}
```

### Autocomplete Implementation

The symbol search uses iced's `combo_box` widget (available in iced 0.13+) for autocomplete. The `combo_box::State` holds the available options and handles filtering internally.

```rust
// Initialized with available symbols (field lives in ToolbarState):
self.toolbar.search_combo_state = combo_box::State::new(self.available_symbols.clone());

// When available_symbols updates:
fn handle_available_symbols_loaded(&mut self, symbols: Vec<String>) {
    self.available_symbols = symbols.clone();
    self.toolbar.search_combo_state = combo_box::State::new(symbols);
}
```

### Toolbar State Flow

```
ToolbarSearchChanged("A")    → filter suggestions to ["AAPL", "AMZN", "AMD", ...]
ToolbarSearchChanged("AA")   → filter to ["AAPL", "AAL", ...]
ToolbarSearchChanged("AAPL") → filter to ["AAPL"]
ToolbarSearchSubmitted       → dispatch ChartSymbolChanged(active_id, "AAPL")
                              → clear search text
                              → unfocus search input

ToolbarTimeframeClicked(M5)  → dispatch ChartTimeframeChanged(active_id, M5)
                              → triggers data reload for new timeframe
```

---

## 9. Theme System

### Custom Theme Architecture

iced 0.14 supports custom themes via the `Theme` type parameter or by implementing styling traits. For Midas, we define a `MidasTheme` struct that provides colors for both iced widgets and the wgpu chart renderer.

```rust
/// Complete color palette for the application.
/// Used by both iced widget styles and wgpu chart shaders.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MidasTheme {
    // ── Application chrome ───────────────────────────────────────
    pub app_background: Color,
    pub toolbar_background: Color,
    pub toolbar_border: Color,
    pub sidebar_background: Color,
    pub statusbar_background: Color,

    // ── Text ─────────────────────────────────────────────────────
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_on_accent: Color,

    // ── Chart rendering (passed to wgpu shaders) ─────────────────
    pub chart_background: Color,
    pub chart_border: Color,
    pub active_chart_border: Color,
    pub grid_color: [f32; 4],
    pub bull_color: [f32; 4],
    pub bear_color: [f32; 4],
    pub volume_bull_color: [f32; 4],
    pub volume_bear_color: [f32; 4],
    pub crosshair_color: [f32; 4],
    pub level_default_color: [f32; 4],
    pub axis_background: Color,
    pub axis_text_color: [f32; 4],

    // ── Interactive elements ─────────────────────────────────────
    pub button_primary_bg: Color,
    pub button_primary_text: Color,
    pub button_secondary_bg: Color,
    pub button_secondary_text: Color,
    pub button_hover_bg: Color,
    pub input_background: Color,
    pub input_border: Color,
    pub input_focused_border: Color,
    pub accent_color: Color,
    pub error_color: Color,
    pub warning_color: Color,
    pub success_color: Color,

    // ── Watchlist ────────────────────────────────────────────────
    pub watchlist_positive: Color,
    pub watchlist_negative: Color,
    pub watchlist_hover: Color,
    pub watchlist_selected: Color,
}

impl MidasTheme {
    /// The default dark theme. Inspired by TC2000/Bloomberg Terminal aesthetics.
    pub fn dark() -> Self {
        Self {
            // Application chrome
            app_background:         Color::from_rgb(0.09, 0.09, 0.11),  // #17171c
            toolbar_background:     Color::from_rgb(0.11, 0.11, 0.14),  // #1c1c24
            toolbar_border:         Color::from_rgb(0.18, 0.18, 0.22),  // #2e2e38
            sidebar_background:     Color::from_rgb(0.10, 0.10, 0.13),  // #1a1a21
            statusbar_background:   Color::from_rgb(0.08, 0.08, 0.10),  // #141419

            // Text
            text_primary:           Color::from_rgb(0.87, 0.87, 0.90),  // #dedee6
            text_secondary:         Color::from_rgb(0.60, 0.60, 0.65),  // #9999a6
            text_muted:             Color::from_rgb(0.40, 0.40, 0.45),  // #666673
            text_on_accent:         Color::from_rgb(1.0, 1.0, 1.0),

            // Chart rendering
            chart_background:       Color::from_rgb(0.07, 0.07, 0.09),  // #121218
            chart_border:           Color::from_rgb(0.15, 0.15, 0.19),  // #262630
            active_chart_border:    Color::from_rgb(0.22, 0.45, 0.85),  // #3873d9
            grid_color:             [1.0, 1.0, 1.0, 0.06],              // Very faint white
            bull_color:             [0.15, 0.75, 0.45, 1.0],            // Green  #26bf73
            bear_color:             [0.85, 0.25, 0.30, 1.0],            // Red    #d9404d
            volume_bull_color:      [0.15, 0.75, 0.45, 0.25],           // Green 25% opacity
            volume_bear_color:      [0.85, 0.25, 0.30, 0.25],           // Red 25% opacity
            crosshair_color:        [0.50, 0.50, 0.55, 0.80],           // Gray 80% opacity
            level_default_color:    [0.85, 0.75, 0.20, 0.90],           // Gold  #d9bf33
            axis_background:        Color::from_rgb(0.09, 0.09, 0.11),
            axis_text_color:        [0.55, 0.55, 0.60, 1.0],

            // Interactive elements
            button_primary_bg:      Color::from_rgb(0.22, 0.45, 0.85),  // Blue accent
            button_primary_text:    Color::WHITE,
            button_secondary_bg:    Color::from_rgb(0.15, 0.15, 0.19),
            button_secondary_text:  Color::from_rgb(0.75, 0.75, 0.80),
            button_hover_bg:        Color::from_rgb(0.20, 0.20, 0.25),
            input_background:       Color::from_rgb(0.12, 0.12, 0.15),
            input_border:           Color::from_rgb(0.22, 0.22, 0.27),
            input_focused_border:   Color::from_rgb(0.22, 0.45, 0.85),
            accent_color:           Color::from_rgb(0.22, 0.45, 0.85),
            error_color:            Color::from_rgb(0.85, 0.25, 0.30),
            warning_color:          Color::from_rgb(0.85, 0.65, 0.20),
            success_color:          Color::from_rgb(0.15, 0.75, 0.45),

            // Watchlist
            watchlist_positive:     Color::from_rgb(0.15, 0.75, 0.45),
            watchlist_negative:     Color::from_rgb(0.85, 0.25, 0.30),
            watchlist_hover:        Color::from_rgb(0.14, 0.14, 0.18),
            watchlist_selected:     Color::from_rgb(0.18, 0.18, 0.24),
        }
    }

    /// Optional light theme.
    pub fn light() -> Self {
        Self {
            app_background:         Color::from_rgb(0.96, 0.96, 0.97),
            toolbar_background:     Color::from_rgb(0.98, 0.98, 0.99),
            toolbar_border:         Color::from_rgb(0.88, 0.88, 0.90),
            sidebar_background:     Color::from_rgb(0.97, 0.97, 0.98),
            statusbar_background:   Color::from_rgb(0.94, 0.94, 0.96),

            text_primary:           Color::from_rgb(0.12, 0.12, 0.15),
            text_secondary:         Color::from_rgb(0.40, 0.40, 0.45),
            text_muted:             Color::from_rgb(0.60, 0.60, 0.65),
            text_on_accent:         Color::WHITE,

            chart_background:       Color::WHITE,
            chart_border:           Color::from_rgb(0.85, 0.85, 0.88),
            active_chart_border:    Color::from_rgb(0.20, 0.40, 0.80),
            grid_color:             [0.0, 0.0, 0.0, 0.06],
            bull_color:             [0.10, 0.60, 0.35, 1.0],
            bear_color:             [0.80, 0.20, 0.25, 1.0],
            volume_bull_color:      [0.10, 0.60, 0.35, 0.20],
            volume_bear_color:      [0.80, 0.20, 0.25, 0.20],
            crosshair_color:        [0.30, 0.30, 0.35, 0.60],
            level_default_color:    [0.70, 0.60, 0.10, 0.90],
            axis_background:        Color::from_rgb(0.96, 0.96, 0.97),
            axis_text_color:        [0.35, 0.35, 0.40, 1.0],

            button_primary_bg:      Color::from_rgb(0.20, 0.40, 0.80),
            button_primary_text:    Color::WHITE,
            button_secondary_bg:    Color::from_rgb(0.92, 0.92, 0.94),
            button_secondary_text:  Color::from_rgb(0.25, 0.25, 0.30),
            button_hover_bg:        Color::from_rgb(0.88, 0.88, 0.91),
            input_background:       Color::WHITE,
            input_border:           Color::from_rgb(0.82, 0.82, 0.85),
            input_focused_border:   Color::from_rgb(0.20, 0.40, 0.80),
            accent_color:           Color::from_rgb(0.20, 0.40, 0.80),
            error_color:            Color::from_rgb(0.80, 0.20, 0.25),
            warning_color:          Color::from_rgb(0.80, 0.60, 0.15),
            success_color:          Color::from_rgb(0.10, 0.60, 0.35),

            watchlist_positive:     Color::from_rgb(0.10, 0.60, 0.35),
            watchlist_negative:     Color::from_rgb(0.80, 0.20, 0.25),
            watchlist_hover:        Color::from_rgb(0.94, 0.94, 0.96),
            watchlist_selected:     Color::from_rgb(0.90, 0.90, 0.93),
        }
    }

    /// Convert an iced Color to a [f32; 4] RGBA array for wgpu shaders.
    pub fn chart_background_rgba(&self) -> [f32; 4] {
        let c = self.chart_background;
        [c.r, c.g, c.b, c.a]
    }
}
```

### Bridging iced Theme with Custom Theme

iced 0.14 uses a `Theme` enum or custom theme type. To apply our `MidasTheme` to iced widgets, we implement iced's styling traits:

```rust
use iced::widget::{button, container, text_input};

/// Convert MidasTheme to iced widget styles.
/// We create style functions that close over our theme colors.
impl MidasApp {
    fn toolbar_container_style(&self) -> impl Fn(&iced::Theme) -> container::Style + '_ {
        move |_theme| container::Style {
            background: Some(self.theme.toolbar_background.into()),
            ..Default::default()
        }
    }
}

// Alternatively, implement a custom iced::Theme that wraps MidasTheme.
// iced 0.14 allows: iced::application("Midas", update, view).theme(|_| custom_theme)
```

### Font Configuration

```rust
// In main.rs, configure fonts before launching iced:
pub fn main() -> iced::Result {
    iced::application("Hand of Midas", MidasApp::update, MidasApp::view)
        .subscription(MidasApp::subscription)
        .theme(MidasApp::theme)
        .font(include_bytes!("../fonts/JetBrainsMono-Regular.ttf").as_slice())
        .default_font(Font {
            family: Family::Name("JetBrains Mono"),
            weight: Weight::Normal,
            stretch: Stretch::Normal,
            style: Style::Normal,
        })
        .antialiasing(false)  // No MSAA — we handle AA in shaders
        .run()
}
```

We use a monospace font (JetBrains Mono or similar) for the entire application. Monospace ensures price labels and axis text align perfectly. The font is embedded in the binary to avoid runtime font loading issues.

---

## 10. Keyboard Shortcuts

### Global vs Chart-Focused Routing

Keyboard events in iced flow through the subscription system (global) and through individual widget `update()` methods (focused). Our design:

1. **Global shortcuts** (handled in subscription): Ctrl+F, Ctrl+1..4, F11, Escape
2. **Chart shortcuts** (routed to active chart): Arrow keys, +/-, Home, End, number keys for timeframe, Delete

The `KeyboardAction` enum (Section 2) abstracts physical keys into semantic actions. The `handle_keyboard_action` method routes them:

```rust
impl MidasApp {
    fn handle_keyboard_action(&mut self, action: KeyAction) -> Task<Message> {
        match action {
            // ── Routed to active chart ───────────────────────────
            KeyAction::ZoomIn => {
                if let Some(id) = self.active_chart {
                    if let Some(chart) = self.charts.get_mut(&id) {
                        let center_x = chart.camera.viewport_width as f32 / 2.0;
                        chart.camera.zoom(center_x, 0.85);
                        chart.dirty.mark_camera();
                        self.trigger_y_autoscale(id);
                        if chart.time_linked {
                            self.propagate_time_axis(id);
                        }
                        self.animating = true;
                    }
                }
                Task::none()
            }
            KeyAction::ZoomOut => {
                if let Some(id) = self.active_chart {
                    if let Some(chart) = self.charts.get_mut(&id) {
                        let center_x = chart.camera.viewport_width as f32 / 2.0;
                        chart.camera.zoom(center_x, 1.18);
                        chart.dirty.mark_camera();
                        self.trigger_y_autoscale(id);
                        if chart.time_linked {
                            self.propagate_time_axis(id);
                        }
                        self.animating = true;
                    }
                }
                Task::none()
            }
            KeyAction::PanLeft => {
                if let Some(id) = self.active_chart {
                    if let Some(chart) = self.charts.get_mut(&id) {
                        let step = chart.camera.time_per_pixel() * 50.0;
                        chart.camera.pan_time(-step);
                        chart.dirty.mark_camera();
                        self.trigger_y_autoscale(id);
                        if chart.time_linked {
                            self.propagate_time_axis(id);
                        }
                        self.animating = true;
                    }
                }
                Task::none()
            }
            KeyAction::PanRight => {
                if let Some(id) = self.active_chart {
                    if let Some(chart) = self.charts.get_mut(&id) {
                        let step = chart.camera.time_per_pixel() * 50.0;
                        chart.camera.pan_time(step);
                        chart.dirty.mark_camera();
                        self.trigger_y_autoscale(id);
                        if chart.time_linked {
                            self.propagate_time_axis(id);
                        }
                        self.animating = true;
                    }
                }
                Task::none()
            }
            KeyAction::JumpToLatest => {
                if let Some(id) = self.active_chart {
                    self.jump_chart_to_latest(id);
                }
                Task::none()
            }
            KeyAction::JumpToOldest => {
                if let Some(id) = self.active_chart {
                    self.jump_chart_to_oldest(id);
                }
                Task::none()
            }
            KeyAction::QuickTimeframe(tf) => {
                if let Some(id) = self.active_chart {
                    return self.handle_timeframe_changed(id, tf);
                }
                Task::none()
            }

            // ── Global shortcuts ─────────────────────────────────
            KeyAction::FocusSymbolSearch => {
                self.toolbar.search_focused = true;
                // Return a focus command for the text input widget.
                // iced 0.14 uses widget::text_input::focus(id) to
                // programmatically focus an input.
                text_input::focus(self.toolbar_search_id.clone())
            }
            KeyAction::LayoutPreset(n) => {
                let preset = match n {
                    1 => LayoutPreset::Single,
                    2 => LayoutPreset::SplitH,
                    3 => LayoutPreset::Grid2x2,
                    4 => LayoutPreset::Grid4x2,
                    _ => return Task::none(),
                };
                self.handle_layout_preset(preset)
            }
            KeyAction::DeleteSelected => {
                if let Some(id) = self.active_chart {
                    if let Some(chart) = self.charts.get(&id) {
                        if let Some(level_id) = chart.selected_level {
                            return self.update(Message::LevelDelete(id, level_id));
                        }
                    }
                }
                Task::none()
            }
            KeyAction::Escape => {
                // Deselect level, unfocus search, cancel any mode
                self.toolbar.search_focused = false;
                if let Some(id) = self.active_chart {
                    if let Some(chart) = self.charts.get_mut(&id) {
                        chart.selected_level = None;
                    }
                }
                Task::none()
            }
            KeyAction::ToggleFrameOverlay => {
                // Toggle frame time overlay (debug/dev feature)
                self.show_frame_overlay = !self.show_frame_overlay;
                Task::none()
            }
        }
    }
}
```

### Search Input Focus Conflict

When the symbol search text input is focused, keyboard events should go to the text input (for typing), not to chart shortcuts. This is handled by checking `toolbar.search_focused` in the keyboard subscription:

```rust
keyboard::on_key_press(|key, modifiers| {
    // If the text input is focused, only intercept Escape and Enter.
    // All other keys pass through to the text input widget.
    // This check happens inside MidasApp via a different mechanism:
    // iced handles focus natively — a focused text_input captures key events
    // before the subscription sees them. So the subscription only receives
    // keys that no focused widget consumed.
    Self::map_key_to_action(key, modifiers)
        .map(Message::KeyboardAction)
})
```

iced's event propagation naturally handles this: a focused `text_input` widget will consume key events in its own `on_event` handler, and only unconsumed events reach the subscription. This means when the search input is focused, typing "AAPL" goes to the input, not to chart shortcuts.

---

## 11. Window Management

### Initial Window Configuration

```rust
pub fn main() -> iced::Result {
    iced::application("Hand of Midas", MidasApp::update, MidasApp::view)
        .subscription(MidasApp::subscription)
        .theme(MidasApp::theme)
        .font(include_bytes!("../fonts/JetBrainsMono-Regular.ttf").as_slice())
        .default_font(Font::with_name("JetBrains Mono"))
        .window(window::Settings {
            size: iced::Size::new(1440.0, 900.0),
            min_size: Some(iced::Size::new(800.0, 500.0)),
            position: window::Position::Centered,
            decorations: true,
            transparent: false,
            icon: Some(load_icon()),  // Application icon
            ..Default::default()
        })
        .antialiasing(false) // We manage AA in shaders
        .run()
}
```

### Window Title

Dynamic title reflecting the active chart's symbol:

```rust
impl MidasApp {
    pub fn title(&self) -> String {
        let symbol_part = self.active_chart
            .and_then(|id| self.charts.get(&id))
            .and_then(|c| c.symbol.as_deref())
            .unwrap_or("No Chart");

        let tf_part = self.active_chart
            .and_then(|id| self.charts.get(&id))
            .map(|c| c.timeframe.display_short())
            .unwrap_or_default();

        if symbol_part == "No Chart" {
            "Hand of Midas".to_string()
        } else {
            format!("{} {} - Hand of Midas", symbol_part, tf_part)
        }
    }
}
```

### Resize Handling

When the window resizes, chart viewports must recalculate. iced handles widget layout automatically through its layout engine — the `Fill` length on chart containers ensures they expand to fill available space. However, the chart's `Camera2D` needs its `viewport_width` and `viewport_height` updated.

This happens in the Shader widget's `Program::draw()` method, which receives the `bounds: Rectangle` parameter reflecting the widget's current size. The viewport dimensions flow through the `ChartRenderSnapshot` (built fresh in `view()` and placed on `ChartProgram`) into `ChartInput`, and `compute_chart_scene()` uses them to compute the projection matrix and pixel-space coordinates in the resulting `ChartScene`:

```rust
// In ChartProgram::draw():
// bounds is available but viewport comes from self.snapshot.
// view() builds ChartProgram with a fresh snapshot each frame,
// including current viewport dimensions from the chart panel state.
let input = ChartInput {
    viewport: &snapshot.viewport,  // includes width, height, margins, dpi_scale
    // ...
};
let scene = compute_chart_scene(&input);
// scene.projection and scene.viewport contain the final rendering geometry.
// Primitive::prepare() reads these to upload the projection UBO.
```

### DPI / Scale Factor Changes

When a user drags the window to a monitor with a different DPI:

1. iced fires a `ScaleFactorChanged` event (captured in our subscription)
2. We update `self.scale_factor`
3. All charts increment their camera generation counter via `dirty.mark_camera()`
4. The next `Program::draw()` picks up the new DPI scale and recalculates pixel-snapped coordinates

```rust
Message::WindowScaleFactorChanged(factor) => {
    self.scale_factor = factor;
    for chart in self.charts.values_mut() {
        chart.camera.dpi_scale = factor as f32;
        chart.dirty.mark_camera();
    }
    Task::none()
}
```

### Future Multi-Window Support

iced 0.14 has experimental multi-window support via the `multi-window` feature flag. The architecture is forward-compatible:

- Each window would have its own `WorkspaceLayout` tree and `pane_chart_map`
- `ChartPanel` instances can be moved between windows
- `MidasApp` remains the single source of truth
- The `view()` function would be called per-window with a window ID
- `Message` variants would include a window ID for window-specific events

This is deferred to Phase 8+, but the current architecture does not preclude it. The `ChartId`-based indirection means a chart can be displayed in any window without structural changes to `ChartPanel`.

---

## 12. Startup Sequence

### Sequence Diagram

```
main()
  │
  ├─ 1. Parse CLI args (optional: --config path, --data-dir path)
  │
  ├─ 2. Initialize tracing/logging subscriber
  │     tracing_subscriber::fmt()
  │       .with_env_filter("midas=debug,wgpu=warn")
  │       .init()
  │
  ├─ 3. Launch iced application
  │     iced::application("Hand of Midas", update, view)
  │       .subscription(subscription)
  │       .theme(theme)
  │       .font(EMBEDDED_FONT)
  │       .window(settings)
  │       .run()
  │
  └─ [iced event loop starts]
       │
       ├─ 4. MidasApp::new() called by iced (or iced::application builder)
       │     │
       │     ├─ Create default MidasTheme::dark()
       │     ├─ Create empty HashMap<ChartId, ChartPanel>
       │     ├─ Create DataManager with data_dir
       │     ├─ Create IndicatorEngine  // Phase 5 — stub in v1
       │     ├─ Set layout = WorkspaceLayout::empty()
       │     ├─ Set pane_chart_map = HashMap::new()
       │     │
       │     └─ Return (MidasApp, Task::batch([
       │            load_config_task,      // 5
       │            scan_symbols_task,     // 6
       │        ]))
       │
       ├─ 5. Async: Load config from data/config.toml
       │     │
       │     ├─ File exists → parse TOML → Message::ConfigLoaded(config)
       │     └─ File missing → Message::ConfigLoaded(AppConfig::default())
       │
       ├─ 6. Async: Scan data directory for available symbols
       │     │
       │     └─ List subdirectories of data/candles/ → Message::AvailableSymbolsLoaded(["AAPL", "SPY", ...])
       │
       ├─ 7. Message::ConfigLoaded(config) arrives
       │     │
       │     ├─ Restore layout (config.layout → self.layout)
       │     ├─ Restore sidebar visibility
       │     ├─ Restore watchlist entries
       │     ├─ Create ChartPanel for each config.charts entry
       │     ├─ Build layout tree, map PaneIds to ChartIds in pane_chart_map
       │     ├─ Set active_chart = first chart
       │     │
       │     └─ Return Task::batch([
       │            load_data_for_chart_0,   // 8
       │            load_data_for_chart_1,   // 8
       │            ...
       │        ])
       │
       ├─ 8. Async (parallel): Load data for each restored chart
       │     │
       │     ├─ DataManager::load_symbol("AAPL", D1) → mmap binary file
       │     ├─ DataManager::load_symbol("SPY", D1)  → mmap binary file
       │     └─ ...
       │
       ├─ 9. Message::DataLoaded(chart_id, buffer) arrives (per chart)
       │     │
       │     ├─ Store data in ChartPanel
       │     ├─ Auto-fit camera to show last 200 candles
       │     ├─ Initialize indicators for this chart
       │     ├─ dirty.mark_data() + dirty.mark_camera()
       │     └─ Chart is now renderable
       │
       ├─ 10. Message::AvailableSymbolsLoaded(symbols) arrives
       │      │
       │      ├─ Store in self.available_symbols
       │      └─ Initialize combo_box::State for autocomplete
       │
       └─ 11. First view() renders:
              │
              ├─ Toolbar (with search input, timeframe buttons)
              ├─ Workspace with chart panels
              │   ├─ Charts with data: Shader widgets render candles
              │   └─ Charts still loading: "Loading..." placeholder
              ├─ Sidebar (if enabled in config)
              └─ Status bar (Disconnected, clock)

              *** Application is now interactive ***
```

### MidasApp Construction (the `new()` function)

```rust
impl MidasApp {
    /// Called once by iced at startup. Returns initial state + startup tasks.
    pub fn new() -> (Self, Task<Message>) {
        let data_dir = Self::resolve_data_dir();

        let app = Self {
            charts: HashMap::new(),
            active_chart: None,
            next_chart_id: 0,

            layout: WorkspaceLayout::empty(),
            pane_chart_map: HashMap::new(),

            sidebar_visible: false,
            watchlist: WatchlistState {
                symbols: Vec::new(),
                scroll_offset: 0.0,
            },

            toolbar: ToolbarState {
                search_text: String::new(),
                search_focused: false,
                search_suggestions: Vec::new(),
                suggestion_index: None,
                search_id: iced::widget::text_input::Id::unique(),
                search_combo_state: combo_box::State::new(Vec::new()),
            },

            data_manager: Arc::new(DataManager::new(data_dir.clone())),
            available_symbols: Vec::new(),

            // TODO(Phase 5): indicator_engine: IndicatorEngine::new(),

            theme: MidasTheme::dark(),

            config: AppConfig::default(),
            config_dirty: false,
            config_save_cooldown: None,

            connection_status: ConnectionStatus::Disconnected,

            animating: false,
            show_frame_overlay: false,
            window_size: (1440, 900),
            scale_factor: 1.0,
        };

        // Startup tasks: load config + scan available symbols
        let config_dir = data_dir.clone();
        let scan_dir = data_dir;

        let startup_tasks = Task::batch([
            // Task 1: Load config
            Task::perform(
                async move {
                    let config_path = config_dir.join("config.toml");
                    match tokio::fs::read_to_string(&config_path).await {
                        Ok(contents) => {
                            toml::from_str::<AppConfig>(&contents)
                                .unwrap_or_default()
                        }
                        Err(_) => AppConfig::default(),
                    }
                },
                Message::ConfigLoaded,
            ),

            // Task 2: Scan for available symbols
            Task::perform(
                async move {
                    let candles_dir = scan_dir.join("candles");
                    let mut symbols = Vec::new();
                    if let Ok(mut entries) = tokio::fs::read_dir(&candles_dir).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                                if let Some(name) = entry.file_name().to_str() {
                                    symbols.push(name.to_string());
                                }
                            }
                        }
                    }
                    symbols.sort();
                    symbols
                },
                Message::AvailableSymbolsLoaded,
            ),
        ]);

        (app, startup_tasks)
    }

    fn resolve_data_dir() -> PathBuf {
        // Check CLI args, env var, then default to ./data/
        std::env::var("MIDAS_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .join("data")
            })
    }
}
```

### Cold Start Performance Target

Per the success criteria in the project overview: **Cold start to first chart rendered < 2 seconds**.

Breakdown:
- Window creation + wgpu surface init: ~200ms
- Config load (TOML parse): ~5ms
- Symbol scan (directory listing): ~10ms
- Binary file mmap: ~1ms per file (no data copying)
- CandleBuffer construction from mmap: ~5ms for 10 years of daily data
- GPU pipeline creation (shader compilation): ~100-200ms (first chart only; cached after)
- First render (instance buffer upload + draw): ~2ms

Total estimated: ~320-420ms for a single chart. Well within the 2-second target.

### Error Recovery at Startup

If config loading fails, the app starts with default state (empty workspace). If data loading fails for a specific chart, that chart enters `LoadState::Error` and displays the error message. The rest of the application remains functional. No startup failure should crash the application — all errors are caught and reported to the user through the UI.

---

## Appendix: iced 0.14 API Summary

### Key API Differences from iced 0.13

iced 0.14 (the target version for this plan) introduced critical Shader API changes:

| Concept | iced 0.13 | iced 0.14 |
|---|---|---|
| Application trait | `iced::application(title, update, view)` builder | Same builder pattern |
| Commands | `Task<Message>` | `Task<Message>` (unchanged) |
| Shader Primitive | `Storage`-based type-keyed map | `Pipeline` associated type (created once, shared) |
| Pipeline init | `storage.has::<T>()` / `storage.store()` / `storage.get()` | `Pipeline::new(device, queue, format)` called by iced |
| Primitive::prepare | `(device, queue, format, &mut Storage, bounds, viewport)` | `(&mut Pipeline, device, queue, bounds, viewport)` |
| Primitive::render | `(encoder, &Storage, target, clip_bounds)` | `(&Pipeline, encoder, target, clip_bounds)` |
| Primitive::draw | Not available | `(&Pipeline, &mut RenderPass) -> bool` (new) |
| wgpu version | 0.19 | 27 |
| Theme | Custom theme via `.theme()` builder | Same |
| Font loading | `.font()` on application builder | Same |
| Subscriptions | `Subscription::run()` / `Subscription::run_with_id()` | Same |
| Window settings | `.window()` on application builder | Same |

### Main Entry Point Pattern (iced 0.14)

```rust
fn main() -> iced::Result {
    iced::application("Hand of Midas", MidasApp::update, MidasApp::view)
        .subscription(MidasApp::subscription)
        .theme(MidasApp::theme)
        .font(include_bytes!("../fonts/JetBrainsMono-Regular.ttf").as_slice())
        .default_font(Font::with_name("JetBrains Mono"))
        .window_size((1440.0, 900.0))
        .antialiasing(false)
        .run()
}
```

### Shader Widget Usage Pattern

```rust
// In view():
shader(my_program)
    .width(Fill)
    .height(Fill)
    .into()

// Where my_program implements shader::Program<Message>
```

### Task Patterns

```rust
// No-op:
Task::none()

// Async operation:
Task::perform(future, map_fn)

// Multiple concurrent tasks:
Task::batch([task1, task2, task3])

// Widget focus:
text_input::focus(id)
```

---

## Appendix: File Map

This plan corresponds to the following source files in the `midas-app` crate:

```
midas-app/src/
├── main.rs              — Entry point, iced application builder (Section 11, 12)
├── app.rs               — MidasApp struct, Message enum, update() (Sections 1, 2, 3)
├── theme.rs             — MidasTheme, color palette, font config (Section 9)
├── views/
│   ├── mod.rs
│   ├── workspace.rs     — Workspace layout, chart grid, split tree (Section 4)
│   ├── toolbar.rs       — Toolbar widget tree, search, timeframes (Section 8)
│   ├── sidebar.rs       — Watchlist panel
│   └── statusbar.rs     — Connection status, clock
└── widgets/
    ├── mod.rs
    └── chart_widget.rs  — ChartProgram, ChartPrimitive, ChartPipeline (Section 5)
```

---

## Appendix: Open Questions and Risks

### Open Questions

1. **RESOLVED: Pipeline sharing across widget instances.** In iced 0.14, the `Pipeline` associated type on `Primitive` is created once and shared across ALL widget instances of the same Primitive type. `SharedPipelines` becomes the `Pipeline` associated type (via `ChartPipeline`), and per-chart state is stored in a `HashMap<ChartId, ChartGpuResources>` inside the Pipeline. No `Storage` type-keyed map is needed. See Section 5 for the complete implementation.

2. **combo_box vs custom overlay for autocomplete**: iced's `combo_box` may not provide enough styling control for a polished autocomplete dropdown. We may need a custom overlay widget. Test with combo_box first; build custom if insufficient.

3. **Shader widget event model**: The exact event forwarding behavior of `shader::Program::update()` in iced 0.14 needs validation. Specifically: does it receive events when the widget is not focused? Does it receive keyboard events? Build a minimal test case in Phase 0.3 to validate.

4. **pane_grid vs manual layout**: The pane_grid widget provides drag-to-resize dividers natively, but may constrain our layout flexibility. Prototype both approaches early and commit to one.

5. **Dirty flag clearing timing**: The generation-counter approach (Section 5) is the canonical solution. Generation counters live in `ChartState` (owned by MidasApp), and the `DirtyTracker` lives in `ChartGpuResources` (inside the Pipeline). No boolean clearing is needed.

### Risks

| Risk | Impact | Mitigation |
|---|---|---|
| iced 0.14 Shader API `draw()` method behavior differs from documentation | Medium | Test in Phase 0.3. The `draw()` method receives a `RenderPass` directly; verify scissor rect behavior. |
| combo_box performance with 10,000+ symbols | Low | Filter client-side, limit displayed suggestions to 20. |
| iced layout engine overhead with 8+ Shader widgets | Medium | Profile in Phase 4. If needed, reduce widget tree depth by combining charts into fewer Shader widgets. |
| wgpu 27 pipeline creation time on first render (~200ms) | Low | Pipeline::new() is called once by iced at startup. Can be hidden behind loading state. |
| ~~Thread safety of `&MidasApp` reference in ChartProgram~~ | ~~Medium~~ | **RESOLVED**: ChartProgram no longer borrows `&MidasApp`. `view()` builds `ChartProgram` with a fresh `ChartRenderSnapshot` each frame. `Program::draw()` reads from `self.snapshot` and calls the pure function `compute_chart_scene()`. No `&MidasApp` reference crosses the widget boundary. |
| wgpu 27 / iced 0.14 version drift | Low | Pin exact versions in Cargo.toml. Run `cargo tree -d \| grep wgpu` after builds to verify no duplicates. |
