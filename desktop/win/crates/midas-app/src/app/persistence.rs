//! Configuration build, save, and debounce logic.

use std::time::Instant;

use iced::widget::pane_grid;
use iced::Task;

use midas_core::config::{
    AccountPanelConfig, AppConfig, ChartConfig, LayoutNode, OrderPanelConfig, PanelSlot,
    ProviderConfig,
};

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
        let mut order_panel_configs: Vec<OrderPanelConfig> = Vec::new();
        let mut account_panel_configs: Vec<AccountPanelConfig> = Vec::new();
        let mut panel_order: Vec<PanelSlot> = Vec::new();
        let mut layout_tree: Vec<LayoutNode> = Vec::new();

        // Walk the pane grid tree to build layout_tree (pre-order traversal).
        let node = self.workspace.panes.layout();
        self.walk_node(
            node,
            &mut chart_configs,
            &mut watchlist_configs,
            &mut order_panel_configs,
            &mut account_panel_configs,
            &mut layout_tree,
        );

        // Also build legacy panel_order from BTreeMap iteration order.
        for ps in self.workspace.panes.panes.values() {
            match &ps.content {
                PanelContent::Chart(chart_id) => {
                    if let Some(idx) = chart_configs.iter().position(|c| {
                        self.charts.get(chart_id).is_some_and(|p| {
                            c.symbol == p.symbol && c.timeframe == p.timeframe.display_name()
                        })
                    }) {
                        panel_order.push(PanelSlot::Chart { chart_index: idx });
                    }
                }
                PanelContent::Watchlist(wl_id) => {
                    if let Some(idx) = watchlist_configs.iter().enumerate().position(|(i, _)| {
                        self.watchlists.get(wl_id).is_some_and(|wl| {
                            watchlist_configs.get(i).is_some_and(
                                |wc: &midas_core::config::WatchlistConfig| wc.name == wl.name,
                            )
                        })
                    }) {
                        panel_order.push(PanelSlot::Watchlist {
                            watchlist_index: idx,
                        });
                    }
                }
                PanelContent::Order(op_id) => {
                    if let Some(idx) = order_panel_configs.iter().enumerate().position(|(i, _)| {
                        self.order_panels.get(op_id).is_some_and(|panel| {
                            order_panel_configs.get(i).is_some_and(|cfg| {
                                cfg.symbol == panel.state.symbol
                                    && cfg.symbol_link == panel.symbol_link
                            })
                        })
                    }) {
                        panel_order.push(PanelSlot::OrderPanel {
                            order_panel_index: idx,
                        });
                    }
                }
                PanelContent::Account(account_id) => {
                    if let Some(idx) =
                        account_panel_configs.iter().enumerate().position(|(i, _)| {
                            self.account_panels.get(account_id).is_some_and(|panel| {
                                account_panel_configs
                                    .get(i)
                                    .is_some_and(|cfg| cfg.name == panel.name)
                            })
                        })
                    {
                        panel_order.push(PanelSlot::Account {
                            account_panel_index: idx,
                        });
                    }
                }
                PanelContent::OrderBlotter(_) => {
                    // Legacy — never populated by this build. Migration
                    // rewrote any persisted slots to Account at load time.
                }
            }
        }

        AppConfig {
            // Always stamp the current schema version on save —
            // load() ensures the in-memory config has been walked
            // forward to current, so anything we write back is at
            // CURRENT_CONFIG_VERSION.
            version: midas_core::config::CURRENT_CONFIG_VERSION,
            // Whole `WindowConfig` projection lives on the
            // controller; the persistence path is just a delegate.
            window: self.window.to_config(),
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
            panel_order,
            layout_tree,
            store: midas_core::config::StoreConfig::default(),
            providers: Some(ProviderConfig {
                active_data: Some(self.providers.active_data_provider_name()),
                active_broker: self.providers.active_broker().map(|b| b.name().to_string()),
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
                                    bound_symbol: panel
                                        .bound_symbol
                                        .as_ref()
                                        .map(|k| k.as_str().to_string()),
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
                        PanelContent::Order(order_id) => {
                            if let Some(panel) = self.order_panels.get(order_id) {
                                let idx = order_panels.len();
                                order_panels.push(panel.to_config());
                                tree.push(LayoutNode::OrderPanel {
                                    order_panel_index: idx,
                                });
                            }
                        }
                        PanelContent::Account(account_id) => {
                            if let Some(panel) = self.account_panels.get(account_id) {
                                let idx = account_panels.len();
                                account_panels.push(panel.to_config());
                                tree.push(LayoutNode::Account {
                                    account_panel_index: idx,
                                });
                            }
                        }
                        PanelContent::OrderBlotter(_) => {
                            // Legacy — never persisted by this build.
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
