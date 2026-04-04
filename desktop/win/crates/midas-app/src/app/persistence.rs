//! Configuration build, save, and debounce logic.

use std::time::Instant;

use iced::Task;

use midas_core::config::{AppConfig, ChartConfig, PanelSlot, ProviderConfig};

use crate::layout::PanelContent;

use super::{Message, MidasApp, CONFIG_SAVE_DEBOUNCE_SECS};

impl MidasApp {
    /// Build an `AppConfig` from the current application state.
    ///
    /// Uses a single pass over workspace panes so that `panel_order` indices
    /// are always consistent with the `charts` and `watchlists` vectors,
    /// even if a pane references a chart/watchlist that no longer exists.
    pub(crate) fn build_config(&self) -> AppConfig {
        let mut chart_configs: Vec<ChartConfig> = Vec::new();
        let mut watchlist_configs = Vec::new();
        let mut panel_order: Vec<PanelSlot> = Vec::new();

        for ps in self.workspace.panes.panes.values() {
            match &ps.content {
                PanelContent::Chart(chart_id) => {
                    if let Some(panel) = self.charts.get(chart_id) {
                        let cam = &panel.chart_state.camera;
                        let idx = chart_configs.len();
                        chart_configs.push(ChartConfig {
                            symbol: panel.symbol.clone(),
                            timeframe: panel.timeframe.display_name().to_string(),
                            levels: vec![], // deprecated — now in top-level levels map
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
                        panel_order.push(PanelSlot::Chart { chart_index: idx });
                    } else {
                        tracing::warn!("Pane references missing {chart_id}, skipping in config");
                    }
                }
                PanelContent::Watchlist(wl_id) => {
                    if let Some(wl) = self.watchlists.get(wl_id) {
                        let idx = watchlist_configs.len();
                        watchlist_configs.push(wl.to_config());
                        panel_order.push(PanelSlot::Watchlist {
                            watchlist_index: idx,
                        });
                    } else {
                        tracing::warn!("Pane references missing {wl_id}, skipping in config");
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
