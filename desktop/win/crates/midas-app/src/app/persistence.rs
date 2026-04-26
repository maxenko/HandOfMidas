//! Configuration build, save, and debounce logic.

use std::collections::BTreeMap;
use std::time::Instant;

use iced::widget::pane_grid;
use iced::Task;

use midas_core::config::{
    AccountPanelConfig, AppConfig, ChartConfig, LayoutNode, OrderPanelConfig, ProviderConfig,
    WindowConfig,
};

use crate::layout::PanelContent;

use super::{Message, MidasApp, CONFIG_SAVE_DEBOUNCE_SECS};

impl MidasApp {
    /// Build an `AppConfig` from the current application state.
    ///
    /// Walks every window's pane-grid `Node` tree to capture the full
    /// layout topology (split axes and ratios). Slice B (v3) emits one
    /// [`WindowConfig`] entry per window into [`AppConfig::windows`];
    /// the legacy top-level `panel_order` and `layout_tree` fields
    /// stay empty (drained by migration on load).
    pub(crate) fn build_config(&self) -> AppConfig {
        let mut chart_configs: Vec<ChartConfig> = Vec::new();
        let mut watchlist_configs = Vec::new();
        let mut order_panel_configs: Vec<OrderPanelConfig> = Vec::new();
        let mut account_panel_configs: Vec<AccountPanelConfig> = Vec::new();
        let mut windows_out: BTreeMap<String, WindowConfig> = BTreeMap::new();

        // Walk every window's pane-grid tree, populating the per-
        // window `layout_tree` and the app-global panel pools as
        // we go.
        for (key, ws) in self.windows.iter() {
            let mut tree: Vec<LayoutNode> = Vec::new();
            self.walk_node(
                ws.layout.panes.layout(),
                &mut chart_configs,
                &mut watchlist_configs,
                &mut order_panel_configs,
                &mut account_panel_configs,
                &mut tree,
            );
            let is_main = *key == self.main_window_key;
            // The main window's authoritative geometry lives in the
            // singleton `WindowGeometry` controller (driven by OS
            // events). Slice C: non-main windows persist their stored
            // `WindowState::geometry` straight through — until the
            // geometry-event subscription gets per-window aware in a
            // later slice, that field is the seed value carried over
            // from load (or from `CreateWindow`'s defaults), which
            // gives a safe-but-static round-trip.
            let geometry = if is_main {
                self.window.to_config()
            } else {
                ws.geometry.clone()
            };
            windows_out.insert(
                key.as_str().to_string(),
                WindowConfig {
                    is_main,
                    geometry,
                    layout_tree: tree,
                },
            );
        }

        // Defensive: ensure the windows map always contains the
        // main entry (matches the validation pass run by `load`).
        windows_out
            .entry(self.main_window_key.as_str().to_string())
            .or_insert_with(|| WindowConfig {
                is_main: true,
                geometry: self.window.to_config(),
                layout_tree: Vec::new(),
            });

        AppConfig {
            // Always stamp the current schema version on save —
            // load() ensures the in-memory config has been walked
            // forward to current, so anything we write back is at
            // CURRENT_CONFIG_VERSION.
            version: midas_core::config::CURRENT_CONFIG_VERSION,
            windows: windows_out,
            theme: midas_core::config::ThemeConfig {
                mode: "dark".into(),
            },
            charts: chart_configs,
            levels: self.annotation_store.to_level_configs(),
            watchlists: watchlist_configs,
            order_panels: order_panel_configs,
            // Legacy vec stays empty — migration drained it at load time
            // and this build only writes `account_panels`.
            order_blotters: Vec::new(),
            account_panels: account_panel_configs,
            recent_symbols: self
                .recent_symbols
                .iter()
                .map(|e| e.symbol.clone())
                .collect(),
            // v3 doesn't write the legacy top-level fields. They
            // skip-serialize when empty so they're absent from the
            // emitted TOML.
            legacy_window: None,
            legacy_panel_order: Vec::new(),
            legacy_layout_tree: Vec::new(),
            store: midas_core::config::StoreConfig::default(),
            providers: Some(ProviderConfig {
                active_data: Some(self.providers.active_data_provider_name()),
                // `active_broker` persistence was tied to the retired
                // `OrderBroker` registry. Route-era connection state
                // is derived from `broker_cfg.backend`; nothing needs
                // to round-trip through the providers section.
                active_broker: None,
            }),
            // Round-trip the broker config so hand-edited TOML
            // (e.g. users switching to LivePaper) survives the
            // next save cycle. Currently the UI doesn't mutate this
            // at runtime; when the settings panel lands, bumps
            // will flow through `app.broker_cfg`.
            broker: self.broker_cfg.clone(),
            // Slice 8a of the chart-transition plan: stamp the
            // chart-view-store schema this binary writes. Drives the
            // R6 rollback coordination — a reverted binary reading a
            // newer stamp than it understands logs a warning and
            // falls back to the v1 reader path.
            chart_view_store_schema: self.chart_views.schema_version(),
        }
    }

    /// Recursively walk the pane grid `Node` tree (pre-order) and populate
    /// the chart/watchlist config vectors and the flattened layout tree.
    ///
    /// Layout-tree leaves emit the panel's stable `id` (slice B / v3),
    /// not its position in the panel-pool vector.
    fn walk_node(
        &self,
        node: &pane_grid::Node,
        charts: &mut Vec<ChartConfig>,
        watchlists: &mut Vec<midas_core::config::WatchlistConfig>,
        order_panels: &mut Vec<OrderPanelConfig>,
        account_panels: &mut Vec<AccountPanelConfig>,
        tree: &mut Vec<LayoutNode>,
    ) {
        match node {
            pane_grid::Node::Split {
                axis, ratio, a, b, ..
            } => {
                let axis_str = match axis {
                    pane_grid::Axis::Horizontal => "horizontal",
                    pane_grid::Axis::Vertical => "vertical",
                };
                tree.push(LayoutNode::Split {
                    axis: axis_str.to_string(),
                    ratio: *ratio,
                });
                self.walk_node(a, charts, watchlists, order_panels, account_panels, tree);
                self.walk_node(b, charts, watchlists, order_panels, account_panels, tree);
            }
            pane_grid::Node::Pane(pane) => {
                // The pane belongs to whichever window's layout we're
                // currently walking — but `pane_to_window` lookups
                // through `panel_to_window` are by panel id, not pane
                // handle. Resolve the pane through whichever window
                // currently contains it. In practice, slice B has one
                // window so this is the main window every time.
                let pane_state = self
                    .windows
                    .values()
                    .find_map(|ws| ws.layout.panes.get(*pane));
                if let Some(ps) = pane_state {
                    match &ps.content {
                        PanelContent::Chart(chart_id) => {
                            if let Some(panel) = self.charts.get(chart_id) {
                                let id = chart_id.0;
                                let cam = &panel.chart_state.camera;
                                charts.push(ChartConfig {
                                    id,
                                    symbol: panel.symbol.clone(),
                                    timeframe: panel.timeframe.display_name().to_string(),
                                    levels: vec![],
                                    camera_time_start: Some(cam.time_start),
                                    camera_time_end: Some(cam.time_end),
                                    camera_price_low: Some(cam.price_low),
                                    camera_price_high: Some(cam.price_high),
                                    collapse_gaps: panel.chart_state.collapse_gaps,
                                    timeline_border_ratio: panel.chart_state.timeline_border_ratio,
                                    volume_scale: panel.chart_state.volume_scale,
                                    show_volume_profile: panel.chart_state.show_volume_profile,
                                    show_levels: panel.chart_state.show_levels,
                                    viewport_width: Some(cam.viewport_width),
                                    viewport_height: Some(cam.viewport_height),
                                    symbol_link: panel.symbol_link,
                                    timeframe_link: panel.timeframe_link,
                                    bound_symbol: panel
                                        .bound_symbol
                                        .as_ref()
                                        .map(|k| k.as_str().to_string()),
                                    // Chart-transition slice 9a: persist the per-panel
                                    // backend selection. `Legacy` is the app-wide
                                    // default and also maps to `None` on load (see
                                    // `ChartPanel::backend_or_default`), so we write
                                    // `None` for `Legacy` to keep existing configs
                                    // byte-identical and only emit the key when the
                                    // user explicitly flipped to `New`.
                                    backend: match panel.backend {
                                        midas_core::ChartBackend::Legacy => None,
                                        other => Some(other),
                                    },
                                });
                                tree.push(LayoutNode::Chart { chart_id: id });
                            }
                        }
                        PanelContent::Watchlist(wl_id) => {
                            if let Some(wl) = self.watchlists.get(wl_id) {
                                watchlists.push(wl.to_config());
                                tree.push(LayoutNode::Watchlist {
                                    watchlist_id: wl_id.0,
                                });
                            }
                        }
                        PanelContent::Order(order_id) => {
                            if let Some(panel) = self.order_panels.get(order_id) {
                                order_panels.push(panel.to_config());
                                tree.push(LayoutNode::OrderPanel {
                                    order_panel_id: order_id.0,
                                });
                            }
                        }
                        PanelContent::Account(account_id) => {
                            if let Some(panel) = self.account_panels.get(account_id) {
                                account_panels.push(panel.to_config());
                                tree.push(LayoutNode::Account {
                                    account_panel_id: account_id.0,
                                });
                            }
                        }
                        PanelContent::Placeholder => {
                            // Placeholder panes are slice-C empty-window
                            // sentinels — not persisted (a freshly-opened
                            // window with no real panels round-trips as
                            // an empty `layout_tree`, which `restore_*`
                            // re-synthesises into a placeholder).
                        }
                        #[cfg(feature = "session_chart")]
                        PanelContent::SessionChart(_) => {
                            // Slice F2: session-chart panes are
                            // session-scoped — they hold a live driver
                            // task + tick subscription that doesn't
                            // round-trip through TOML. Treat them like
                            // a placeholder for persistence purposes;
                            // the user reopens the chart from the
                            // toolbar after restart, mirroring the
                            // legacy floating-window behaviour.
                        }
                    }
                }
            }
        }
    }

    /// Mark the configuration as dirty so it will be saved on the next tick.
    pub(crate) fn mark_config_dirty(&mut self) {
        self.config_dirty = true;
    }

    /// Save the configuration if dirty and debounce interval has elapsed.
    pub(crate) fn maybe_save_config(&mut self) -> Task<Message> {
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
    ///
    /// Bracket annotations are now persisted via `TickerStatePersistHandle`
    /// (redb v2), not JSON files. The JSON write path has been removed.
    pub(crate) fn flush_config(&mut self) -> Task<Message> {
        self.config_dirty = false;
        self.last_config_save = Instant::now();
        let config = self.build_config();
        let path = self.config_path.clone();

        Task::perform(
            async move {
                let result = tokio::task::spawn_blocking(move || config.save(&path)).await;
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
