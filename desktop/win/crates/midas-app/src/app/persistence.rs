//! Configuration build, save, and debounce logic.

use std::time::Instant;

use iced::widget::pane_grid;
use iced::Task;

use midas_core::config::{AppConfig, ChartConfig, LayoutNode, PanelSlot, ProviderConfig};

use crate::layout::PanelContent;

use super::{Message, MidasApp, CONFIG_SAVE_DEBOUNCE_SECS};

impl MidasApp {
    /// Build an `AppConfig` from the current application state.
    ///
    /// Walks the pane grid `Node` tree to capture the full layout topology
    /// (split axes and ratios). Also builds the legacy `panel_order` for
    /// backward compatibility.
    pub(crate) fn build_config(&self) -> AppConfig {
        let mut chart_configs: Vec<ChartConfig> = Vec::new();
        let mut watchlist_configs = Vec::new();
        let mut panel_order: Vec<PanelSlot> = Vec::new();
        let mut layout_tree: Vec<LayoutNode> = Vec::new();

        // Walk the pane grid tree to build layout_tree (pre-order traversal).
        let node = self.workspace.panes.layout();
        self.walk_node(
            node,
            &mut chart_configs,
            &mut watchlist_configs,
            &mut layout_tree,
        );

        // Also build legacy panel_order from BTreeMap iteration order.
        for ps in self.workspace.panes.panes.values() {
            match &ps.content {
                PanelContent::Chart(chart_id) => {
                    if let Some(idx) = chart_configs
                        .iter()
                        .position(|c| self.charts.get(chart_id).map_or(false, |p| {
                            c.symbol == p.symbol && c.timeframe == p.timeframe.display_name()
                        }))
                    {
                        panel_order.push(PanelSlot::Chart { chart_index: idx });
                    }
                }
                PanelContent::Watchlist(wl_id) => {
                    if let Some(idx) = watchlist_configs
                        .iter()
                        .enumerate()
                        .position(|(i, _)| {
                            self.watchlists
                                .get(wl_id)
                                .map_or(false, |wl| {
                                    watchlist_configs.get(i).map_or(false, |wc: &midas_core::config::WatchlistConfig| {
                                        wc.name == wl.name
                                    })
                                })
                        })
                    {
                        panel_order.push(PanelSlot::Watchlist {
                            watchlist_index: idx,
                        });
                    }
                }
            }
        }

        let (win_w, win_h) = self.window_size;
        let (win_x, win_y) = self.window_position.unzip();

        AppConfig {
            window: midas_core::config::WindowConfig {
                width: win_w,
                height: win_h,
                maximized: false,
                x: win_x,
                y: win_y,
                monitor_width: self.monitor_size.map(|(w, _)| w),
                monitor_height: self.monitor_size.map(|(_, h)| h),
            },
            theme: midas_core::config::ThemeConfig {
                mode: "dark".into(),
            },
            charts: chart_configs,
            levels: self.level_store.to_config(),
            watchlists: watchlist_configs,
            panel_order,
            layout_tree,
            store: midas_core::config::StoreConfig::default(),
            providers: Some(ProviderConfig {
                active_data: Some(self.providers.active_data_provider_name()),
                active_broker: self
                    .providers
                    .active_broker()
                    .map(|b| b.name().to_string()),
            }),
        }
    }

    /// Recursively walk the pane grid `Node` tree (pre-order) and populate
    /// the chart/watchlist config vectors and the flattened layout tree.
    fn walk_node(
        &self,
        node: &pane_grid::Node,
        charts: &mut Vec<ChartConfig>,
        watchlists: &mut Vec<midas_core::config::WatchlistConfig>,
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
                self.walk_node(a, charts, watchlists, tree);
                self.walk_node(b, charts, watchlists, tree);
            }
            pane_grid::Node::Pane(pane) => {
                if let Some(ps) = self.workspace.panes.get(*pane) {
                    match &ps.content {
                        PanelContent::Chart(chart_id) => {
                            if let Some(panel) = self.charts.get(chart_id) {
                                let cam = &panel.chart_state.camera;
                                let idx = charts.len();
                                charts.push(ChartConfig {
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
                                });
                                tree.push(LayoutNode::Chart { chart_index: idx });
                            }
                        }
                        PanelContent::Watchlist(wl_id) => {
                            if let Some(wl) = self.watchlists.get(wl_id) {
                                let idx = watchlists.len();
                                watchlists.push(wl.to_config());
                                tree.push(LayoutNode::Watchlist {
                                    watchlist_index: idx,
                                });
                            }
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
    /// Also persists bracket annotations alongside the config file.
    pub(crate) fn flush_config(&mut self) -> Task<Message> {
        self.config_dirty = false;
        self.last_config_save = Instant::now();
        let config = self.build_config();
        let path = self.config_path.clone();

        // Save annotations alongside config.
        let data_dir = self
            .config_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        if let Err(e) =
            crate::annotation_persistence::save_all(&self.annotation_store, &data_dir)
        {
            tracing::warn!("Failed to persist annotations: {e}");
        }

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
