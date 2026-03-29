//! Configuration build, save, and debounce logic.

use std::time::Instant;

use iced::Task;

use midas_core::config::{AppConfig, ChartConfig, LevelConfig};

use super::{Message, MidasApp, CONFIG_SAVE_DEBOUNCE_SECS};

impl MidasApp {
    /// Build an `AppConfig` from the current application state.
    pub(crate) fn build_config(&self) -> AppConfig {
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
                        label: l.label.clone(),
                        icon: l.icon.to_str_id().to_string(),
                        locked: l.locked,
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
                    timeline_border_ratio: panel.chart_state.timeline_border_ratio,
                    volume_scale: panel.chart_state.volume_scale,
                    show_volume_profile: panel.chart_state.show_volume_profile,
                    show_levels: panel.chart_state.show_levels,
                    viewport_width: Some(cam.viewport_width),
                    viewport_height: Some(cam.viewport_height),
                }
            })
            .collect();

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
