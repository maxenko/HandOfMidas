//! View functions for the main application.
//!
//! Builds the widget tree: toolbar, pane grid, title bars, chart body,
//! status bar, and floating chart windows.

use std::sync::Arc;

use iced::widget::pane_grid::{self, PaneGrid};
use iced::widget::{button, column, container, row, stack, text, text_input, Column, Row, Space};
use iced::{window, Color, Element, Fill, Length};

use midas_core::{ChartId, Timeframe};

use crate::theme;

use super::{ChartPanel, LoadState, Message, MidasApp};

// ── Main entry point ────────────────────────────────────────────────

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
    fn view_floating_chart<'a>(&'a self, chart: &'a ChartPanel) -> Element<'a, Message> {
        // If data is loaded, render via GPU Shader widget.
        if let (LoadState::Loaded, Some(ref data)) = (&chart.load_state, &chart.data) {
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
                timeline_border_ratio: chart.chart_state.timeline_border_ratio,
                volume_scale: chart.chart_state.volume_scale,
                show_volume_profile: chart.chart_state.show_volume_profile,
                data_time_start: chart.chart_state.data_time_start,
                data_time_end: chart.chart_state.data_time_end,
                editing_level_id: chart.editing_level_id,
                level_tool: chart.chart_state.level_tool.clone(),
            };
            // Use ChartId(0) for floating windows -- they don't participate
            // in the pane_grid's chart map.
            let program = crate::chart_widget::ChartProgram {
                chart_id: ChartId::new(0),
                snapshot,
            };
            let shader = crate::chart_widget::chart_shader(program);

            // Compute date labels for the time axis overlay.
            let camera = &chart.chart_state.camera;
            let candle_duration = midas_chart::estimate_candle_duration(data.as_ref());
            let date_labels = midas_chart::date_labels::compute(
                camera,
                data.as_ref(),
                candle_duration,
                chart.chart_state.collapse_gaps,
            );
            let date_overlay = build_date_label_overlay(
                &date_labels,
                camera,
                chart.chart_state.timeline_border_ratio,
            );

            let price_overlay = build_price_label_overlay(
                camera,
                chart.chart_state.timeline_border_ratio,
            );

            // Build level-related overlays for floating window.
            let floating_chart_id = ChartId::new(0);
            let level_renders = compute_level_renders(chart);
            let level_labels_overlay = build_level_labels_overlay(
                &level_renders,
                chart.chart_state.camera.viewport_height,
            );
            let is_placing = chart.chart_state.level_tool.is_placing();
            let drawing_panel = build_drawing_panel(floating_chart_id, is_placing);

            let mut chart_layers: Vec<Element<'_, Message>> = vec![
                shader.into(),
                date_overlay,
                price_overlay,
                level_labels_overlay,
                drawing_panel,
            ];

            // Level editor popup (when a level is being edited).
            if let (Some(editing_id), Some(screen_pos)) =
                (chart.editing_level_id, chart.editing_level_screen_pos)
            {
                if let Some(level) = chart.chart_state.levels.iter().find(|l| l.id == editing_id) {
                    chart_layers.push(build_level_editor(
                        floating_chart_id,
                        level,
                        screen_pos,
                        &chart.level_editor_price_input,
                        chart.chart_state.camera.viewport_width,
                        chart.chart_state.camera.viewport_height,
                    ));
                }
            }

            let chart_area = stack(chart_layers).width(Fill).height(Fill);

            // Header bar with symbol and timeframe.
            let header = container(
                row![
                    text(&chart.symbol).size(13).color(Color::WHITE),
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

            return column![header, chart_area].into();
        }

        // No data placeholder for floating window.
        let status_text = match &chart.load_state {
            LoadState::Empty => "No data loaded".to_string(),
            LoadState::Loading => "Loading...".to_string(),
            LoadState::Loaded => "Loaded".to_string(),
            LoadState::Error(e) => format!("Error: {e}"),
        };
        container(text(status_text).size(14).color(theme::TEXT_SECONDARY))
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
}

// ── Toolbar ─────────────────────────────────────────────────────────

impl MidasApp {
    /// Build the toolbar row (layout presets, split actions, add-chart).
    fn view_toolbar(&self) -> Element<'_, Message> {
        let layout_buttons = row![
            button(text("1").size(12))
                .on_press(Message::LayoutPreset(
                    crate::layout::LayoutPresetKind::Single
                ))
                .padding([4, 8])
                .style(hover_text_button_style),
            button(text("1|1").size(12))
                .on_press(Message::LayoutPreset(
                    crate::layout::LayoutPresetKind::SplitH
                ))
                .padding([4, 8])
                .style(hover_text_button_style),
            button(text("1/1").size(12))
                .on_press(Message::LayoutPreset(
                    crate::layout::LayoutPresetKind::SplitV
                ))
                .padding([4, 8])
                .style(hover_text_button_style),
            button(text("2x2").size(12))
                .on_press(Message::LayoutPreset(
                    crate::layout::LayoutPresetKind::Grid2x2
                ))
                .padding([4, 8])
                .style(hover_text_button_style),
        ]
        .spacing(2);

        let split_buttons = row![
            button(text("Split H").size(11))
                .on_press_maybe(
                    self.workspace
                        .focus
                        .map(|p| { Message::PaneSplit(pane_grid::Axis::Horizontal, p) })
                )
                .padding([4, 6])
                .style(hover_text_button_style),
            button(text("Split V").size(11))
                .on_press_maybe(
                    self.workspace
                        .focus
                        .map(|p| { Message::PaneSplit(pane_grid::Axis::Vertical, p) })
                )
                .padding([4, 6])
                .style(hover_text_button_style),
        ]
        .spacing(2);

        let add_btn = button(text("+").size(14))
            .on_press(Message::AddChart)
            .padding([4, 10])
            .style(hover_text_button_style);

        let toolbar_row = row![layout_buttons, split_buttons, add_btn,]
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
}

// ── Pane grid content ───────────────────────────────────────────────

impl MidasApp {
    /// Build the main content area using iced's pane_grid widget.
    fn view_content(&self) -> Element<'_, Message> {
        let focused_pane = self.workspace.focus;
        let pane_count = self.workspace.pane_count();

        let pane_grid_widget =
            PaneGrid::new(&self.workspace.panes, |pane, pane_state, _is_maximized| {
                let is_focused = focused_pane == Some(pane);
                let chart_id = pane_state.chart_id;

                let title_bar = self.view_pane_title_bar(chart_id, pane, pane_count);

                let body = self.view_pane_body(chart_id);
                // Content style: dark background (serves as title bar bg
                // since TitleBar is transparent) + focus border.
                pane_grid::Content::new(body)
                    .title_bar(title_bar)
                    .style(move |_theme| {
                        let border_color = if is_focused {
                            theme::CHART_ACTIVE_BORDER
                        } else {
                            theme::CHART_INACTIVE_BORDER
                        };
                        container::Style {
                            background: Some(iced::Background::Color(Color::from_rgb(
                                0.06, 0.08, 0.12,
                            ))),
                            border: iced::Border {
                                color: border_color,
                                width: if is_focused { 2.0 } else { 1.0 },
                                radius: 0.0.into(),
                            },
                            ..Default::default()
                        }
                    })
            })
            .on_resize(6, Message::PaneResized)
            .on_drag(Message::PaneDragged)
            .style(|_theme| pane_grid::Style {
                hovered_region: pane_grid::Highlight {
                    background: iced::Background::Color(Color::from_rgba(0.2, 0.4, 0.8, 0.25)),
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
}

// ── Title bar ───────────────────────────────────────────────────────

impl MidasApp {
    /// Build the TitleBar for a pane.
    ///
    /// Layout: `[TICKER][1m|5m|...][G][R] [..drag area..] [⧉][×]`
    fn view_pane_title_bar(
        &self,
        chart_id: ChartId,
        pane: pane_grid::Pane,
        pane_count: usize,
    ) -> pane_grid::TitleBar<'_, Message> {
        // iced's TitleBar drag zone = title bar area NOT covered by content
        // bounds or controls bounds. Buttons in content still capture clicks.
        let title_content = self.view_title_bar_content(chart_id);
        let controls_row = self.view_title_bar_controls(pane, pane_count);

        pane_grid::TitleBar::new(title_content)
            .controls(controls_row)
            .padding([2, 4])
            .always_show_controls()
            // Transparent — Content's background + focus border show through.
            .style(|_theme| container::Style::default())
    }

    /// Build the content (left) area of a pane's TitleBar.
    fn view_title_bar_content(&self, chart_id: ChartId) -> Element<'_, Message> {
        let chart = self.charts.get(&chart_id);
        let panel_tf = chart.map(|c| c.timeframe).unwrap_or(Timeframe::D1);
        let symbol_input_value = chart.map(|c| c.symbol_input.as_str()).unwrap_or("");

        let ticker_input = text_input("SYMBOL", symbol_input_value)
            .on_input(move |val| Message::PanelSymbolInputChanged(chart_id, val))
            .on_submit(Message::PanelSymbolSubmitted(chart_id))
            .width(70)
            .size(11)
            .padding([2, 4]);

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

        let collapse_active = chart.map(|c| c.chart_state.collapse_gaps).unwrap_or(false);
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

        let vp_active = chart
            .map(|c| c.chart_state.show_volume_profile)
            .unwrap_or(false);
        let vp_btn = if vp_active {
            button(text("VP").size(10).color(Color::WHITE))
                .on_press(Message::ToggleVolumeProfile(chart_id))
                .padding([1, 4])
                .style(button::primary)
        } else {
            button(text("VP").size(10))
                .on_press(Message::ToggleVolumeProfile(chart_id))
                .padding([1, 4])
                .style(button::text)
        };

        let reset_btn = button(text("R").size(10))
            .on_press(Message::ResetChart(chart_id))
            .padding([1, 4])
            .style(button::text);

        row![ticker_input, tf_row, collapse_btn, vp_btn, reset_btn]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .height(24)
            .into()
    }

    /// Build the controls (right) area of a pane's TitleBar.
    fn view_title_bar_controls(
        &self,
        pane: pane_grid::Pane,
        pane_count: usize,
    ) -> Element<'_, Message> {
        let pop_out_btn = button(text("\u{29C9}").size(12))
            .on_press(Message::PopOut(pane))
            .padding([1, 5])
            .style(button::text);

        let close_btn: Element<'_, Message> = if pane_count > 1 {
            button(text("\u{00D7}").size(12))
                .on_press(Message::PaneClose(pane))
                .padding([1, 5])
                .style(button::text)
                .into()
        } else {
            Space::new().width(0).height(0).into()
        };

        row![pop_out_btn, close_btn]
            .spacing(2)
            .align_y(iced::Alignment::Center)
            .into()
    }
}

// ── Pane body ───────────────────────────────────────────────────────

impl MidasApp {
    /// Render the body content of a single pane (chart or placeholder).
    fn view_pane_body(&self, chart_id: ChartId) -> Element<'_, Message> {
        let chart = match self.charts.get(&chart_id) {
            Some(c) => c,
            None => return self.view_empty_placeholder(),
        };

        if let (LoadState::Loaded, Some(ref data)) = (&chart.load_state, &chart.data) {
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
                timeline_border_ratio: chart.chart_state.timeline_border_ratio,
                volume_scale: chart.chart_state.volume_scale,
                show_volume_profile: chart.chart_state.show_volume_profile,
                data_time_start: chart.chart_state.data_time_start,
                data_time_end: chart.chart_state.data_time_end,
                editing_level_id: chart.editing_level_id,
                level_tool: chart.chart_state.level_tool.clone(),
            };
            let program = crate::chart_widget::ChartProgram { chart_id, snapshot };
            let shader = crate::chart_widget::chart_shader(program);

            // Compute date labels for the time axis overlay.
            let camera = &chart.chart_state.camera;
            let candle_duration = midas_chart::estimate_candle_duration(data.as_ref());
            let date_labels = midas_chart::date_labels::compute(
                camera,
                data.as_ref(),
                candle_duration,
                chart.chart_state.collapse_gaps,
            );

            let date_overlay = build_date_label_overlay(
                &date_labels,
                camera,
                chart.chart_state.timeline_border_ratio,
            );

            let price_overlay = build_price_label_overlay(
                camera,
                chart.chart_state.timeline_border_ratio,
            );

            // Build level-related overlays.
            let level_renders = compute_level_renders(chart);
            let level_labels_overlay = build_level_labels_overlay(
                &level_renders,
                chart.chart_state.camera.viewport_height,
            );
            let is_placing = chart.chart_state.level_tool.is_placing();
            let drawing_panel = build_drawing_panel(chart_id, is_placing);

            let mut chart_layers: Vec<Element<'_, Message>> = vec![
                shader.into(),
                date_overlay,
                price_overlay,
                level_labels_overlay,
                drawing_panel,
            ];

            // Level editor popup (when a level is being edited).
            if let (Some(editing_id), Some(screen_pos)) =
                (chart.editing_level_id, chart.editing_level_screen_pos)
            {
                if let Some(level) = chart.chart_state.levels.iter().find(|l| l.id == editing_id) {
                    chart_layers.push(build_level_editor(
                        chart_id,
                        level,
                        screen_pos,
                        &chart.level_editor_price_input,
                        chart.chart_state.camera.viewport_width,
                        chart.chart_state.camera.viewport_height,
                    ));
                }
            }

            return container(stack(chart_layers).width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .padding(2) // Inset so Content's focus border is visible.
                .into();
        }

        // Placeholder for empty/loading/error states.
        let status_text = match &chart.load_state {
            LoadState::Empty => "No data -- type a symbol and press Enter".to_string(),
            LoadState::Loading => "Loading...".to_string(),
            LoadState::Loaded => "Loaded".to_string(),
            LoadState::Error(e) => format!("Error: {e}"),
        };
        let bg_color = theme::CHART_EMPTY_BG;

        container(text(status_text).size(14).color(theme::TEXT_SECONDARY))
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill)
            .padding(2) // Inset so Content's focus border is visible.
            .style(move |_theme| container::Style {
                background: Some(bg_color.into()),
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
}

// ── Status bar ──────────────────────────────────────────────────────

impl MidasApp {
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

// ── Button style helpers ────────────────────────────────────────────

/// Build an iced widget overlay for date labels at the separator line.
///
/// Uses `FillPortion`-based positioning so labels scale correctly
/// regardless of actual widget bounds. Horizontal gaps are also expressed
/// as fill-portions relative to the camera viewport width.
fn build_date_label_overlay<'a>(
    labels: &[midas_chart::DateLabel],
    camera: &midas_chart::camera::Camera2D,
    timeline_border_ratio: f32,
) -> Element<'a, Message> {
    let label_font_size = 10.0;
    let secondary_font_size = 9.0;
    let char_width = 5.5_f32;
    let vw = camera.viewport_width.max(1) as f32;

    // We express horizontal positions as FillPortion values (integers)
    // so the row scales proportionally to actual widget width.
    let portion_scale = 1000.0 / vw;

    // Two independent rows: time labels above the separator, date labels below.
    // This keeps time text at a fixed vertical position regardless of whether
    // a date label is present.
    let mut time_row = Row::new();
    let mut time_cursor = 0.0_f32;

    let mut date_row = Row::new();
    let mut date_cursor = 0.0_f32;

    for dl in labels {
        let text_width = dl.text.len() as f32 * char_width;
        let target_x = (dl.screen_x - text_width / 2.0).max(0.0);
        let gap = target_x - time_cursor;

        if gap > 1.0 {
            let portion = ((gap * portion_scale) as u16).max(1);
            time_row = time_row.push(Space::new().width(Length::FillPortion(portion)));
            time_cursor += gap;
        } else if gap < -1.0 {
            // Labels overlap — skip.
            continue;
        }

        let label_color = if dl.is_boundary {
            Color::from_rgb(0.75, 0.75, 0.75)
        } else {
            Color::from_rgb(0.55, 0.55, 0.55)
        };

        time_row = time_row.push(
            text(dl.text.clone())
                .size(label_font_size)
                .color(label_color),
        );
        time_cursor += text_width;

        // Place secondary (date) text in the separate date row below.
        if let Some(ref secondary) = dl.secondary {
            let sec_width = secondary.len() as f32 * (char_width * 0.9);
            let sec_target_x = (dl.screen_x - sec_width / 2.0).max(0.0);
            let sec_gap = sec_target_x - date_cursor;

            if sec_gap > 1.0 {
                let portion = ((sec_gap * portion_scale) as u16).max(1);
                date_row = date_row.push(Space::new().width(Length::FillPortion(portion)));
                date_cursor += sec_gap;
            }

            date_row = date_row.push(
                text(secondary.clone())
                    .size(secondary_font_size)
                    .color(Color::from_rgb(0.65, 0.65, 0.65)),
            );
            date_cursor += sec_width;
        }
    }

    // Trailing spacers so labels don't stretch to fill.
    let time_remaining = vw - time_cursor;
    if time_remaining > 1.0 {
        let portion = ((time_remaining * portion_scale) as u16).max(1);
        time_row = time_row.push(Space::new().width(Length::FillPortion(portion)));
    }
    let date_remaining = vw - date_cursor;
    if date_remaining > 1.0 {
        let portion = ((date_remaining * portion_scale) as u16).max(1);
        date_row = date_row.push(Space::new().width(Length::FillPortion(portion)));
    }

    // Fixed heights so the FillPortion math is stable regardless
    // of whether a date label is present.
    let _time_row_height = label_font_size + 2.0;
    let _date_row_height = secondary_font_size + 2.0;

    // Time row anchored at the bottom of the price area (above border).
    // Date row anchored at the top of the volume area (below border).
    // Neither row's content affects the other's position.
    // Fixed pixel heights match the GPU separator line exactly.
    let vh = camera.viewport_height.max(1) as f32;
    let border_y = vh * (1.0 - timeline_border_ratio);
    container(
        column![
            container(column![
                Space::new().width(Fill).height(Fill),
                time_row,
                Space::new().height(Length::Fixed(4.0)),
            ])
            .width(Fill)
            .height(Length::Fixed(border_y)),
            container(column![Space::new().height(Length::Fixed(4.0)), date_row,])
                .width(Fill)
                .height(Length::Fixed(vh - border_y)),
        ]
        .width(Fill)
        .height(Fill),
    )
    .width(Fill)
    .height(Fill)
    .into()
}

/// Build an iced widget overlay for price labels along the right edge of
/// the chart's price area. Labels are positioned at the same Y coordinates
/// as the horizontal grid lines, using fixed-pixel spacers for exact alignment.
///
/// Uses the same font size (10pt) and muted-gray color as the time axis labels
/// so the two scales look consistent.
fn build_price_label_overlay<'a>(
    camera: &midas_chart::camera::Camera2D,
    timeline_border_ratio: f32,
) -> Element<'a, Message> {
    let label_font_size = 10.0;
    let label_height = label_font_size + 2.0;
    let vh = camera.viewport_height.max(1) as f32;
    let border_y = vh * (1.0 - timeline_border_ratio);

    let labels = midas_chart::compute_y_labels(camera);

    // Filter to labels within the price area and sort top-to-bottom.
    let mut visible: Vec<_> = labels
        .iter()
        .filter(|l| l.screen_y >= label_height / 2.0 && l.screen_y < border_y - label_height / 2.0)
        .collect();
    visible.sort_by(|a, b| a.screen_y.partial_cmp(&b.screen_y).unwrap_or(std::cmp::Ordering::Equal));

    // Build a column with fixed-height spacers between right-aligned labels.
    let mut col = Column::new();
    let mut cursor_y = 0.0_f32;
    let label_color = Color::from_rgb(0.55, 0.55, 0.55);

    for label in &visible {
        let target_y = (label.screen_y - label_height / 2.0).max(0.0);
        let gap = target_y - cursor_y;

        if gap < 1.0 {
            // Labels would overlap — skip.
            continue;
        }

        col = col.push(Space::new().height(Length::Fixed(gap)));
        cursor_y += gap;

        col = col.push(
            container(
                row![
                    Space::new().width(Fill),
                    text(label.text.clone()).size(label_font_size).color(label_color),
                    Space::new().width(Length::Fixed(4.0)),
                ]
                .width(Fill),
            )
            .width(Fill),
        );
        cursor_y += label_height;
    }

    // Trailing spacer absorbs remaining height.
    col = col.push(Space::new().height(Fill));

    container(col.width(Fill))
        .width(Fill)
        .height(Length::Fixed(border_y))
        .into()
}

// ── Drawing panel overlay ──────────────────────────────────────────

/// Build the drawing-tools panel that floats at the top-left of the chart.
///
/// Contains a single "Level" button that enters level-placement mode.
/// When `is_placing` is true the button is highlighted to indicate the
/// active tool.
fn build_drawing_panel<'a>(chart_id: ChartId, is_placing: bool) -> Element<'a, Message> {
    let bg_color = if is_placing {
        Color::from_rgba(0.22, 0.55, 0.95, 0.85) // Blue highlight when active
    } else {
        Color::from_rgba(0.15, 0.17, 0.22, 0.85)
    };
    let border_color = if is_placing {
        Color::from_rgba(0.3, 0.5, 0.9, 0.7)
    } else {
        Color::from_rgba(0.3, 0.3, 0.4, 0.5)
    };

    let level_btn = button(
        row![
            text("\u{2500}").size(14), // horizontal line
            text("Level").size(11),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::DrawingPanelCreateLevel(chart_id))
    .padding([4, 10])
    .style(move |_theme: &iced::Theme, _status| button::Style {
        background: Some(iced::Background::Color(bg_color)),
        border: iced::Border {
            color: border_color,
            width: 1.0,
            radius: 6.0.into(),
        },
        text_color: Color::from_rgba(0.8, 0.8, 0.85, 0.9),
        ..Default::default()
    });

    let clear_btn = button(
        row![
            text("\u{00D7}").size(14), // ×
            text("Clear").size(11),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::ChartClearAllLevels(chart_id))
    .padding([4, 10])
    .style(|_theme: &iced::Theme, _status| button::Style {
        background: Some(iced::Background::Color(Color::from_rgba(
            0.15, 0.17, 0.22, 0.85,
        ))),
        border: iced::Border {
            color: Color::from_rgba(0.3, 0.3, 0.4, 0.5),
            width: 1.0,
            radius: 6.0.into(),
        },
        text_color: Color::from_rgba(0.8, 0.8, 0.85, 0.9),
        ..Default::default()
    });

    container(column![level_btn, clear_btn].spacing(4))
        .padding(iced::Padding::ZERO.top(40.0).left(8.0))
        .width(Fill)
        .height(Fill)
        .into()
}

// ── Level labels overlay ───────────────────────────────────────────

/// Build an overlay that renders text labels for levels that have labels
/// or icons set. Each label is positioned at the level's Y coordinate
/// on the chart, appearing as a small badge near the left side.
fn build_level_labels_overlay<'a>(
    levels: &[midas_chart::LevelRender],
    _viewport_height: u32,
) -> Element<'a, Message> {
    let mut label_elements: Vec<Element<'a, Message>> = Vec::new();

    for level in levels {
        let label_str = match (&level.label, level.icon.as_char()) {
            (Some(lbl), Some(icon_ch)) if !lbl.is_empty() => {
                format!("{} {}", icon_ch, lbl)
            }
            (Some(lbl), None) if !lbl.is_empty() => lbl.clone(),
            (None, Some(icon_ch)) | (Some(_), Some(icon_ch)) => icon_ch.to_string(),
            _ => continue,
        };

        let [r, g, b, a] = level.color;
        let label_color = Color::from_rgba(r, g, b, a.max(0.9));
        let bg_color = Color::from_rgba(r * 0.3, g * 0.3, b * 0.3, 0.75);

        // Center the badge vertically on the level line.
        // Badge height ≈ font_size(16) + vertical_padding(3*2) + border ≈ 24px.
        let badge_half_height = 14.0;
        let top_pad = (level.screen_y - badge_half_height).max(0.0);

        let label_widget = container(text(label_str).size(16).color(label_color))
            .padding([3, 8])
            .style(move |_theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(bg_color)),
                border: iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        let positioned = container(label_widget)
            .padding(iced::Padding::ZERO.top(top_pad).left(8.0))
            .width(Fill)
            .height(Fill);

        label_elements.push(positioned.into());
    }

    if label_elements.is_empty() {
        return Space::new().width(0).height(0).into();
    }

    // Stack all labels on top of each other (each positions itself via
    // top padding).
    stack(label_elements).width(Fill).height(Fill).into()
}

// ── Level editor popup ─────────────────────────────────────────────

/// Build the floating level-editor popup that appears on right-click.
///
/// Contains price input with step buttons, label input, color presets,
/// thickness buttons, icon selector, lock toggle, and delete button.
fn build_level_editor<'a>(
    chart_id: ChartId,
    level: &midas_chart::HorizontalLevel,
    screen_pos: (f32, f32),
    price_input: &str,
    viewport_width: u32,
    viewport_height: u32,
) -> Element<'a, Message> {
    let level_id = level.id;
    let (coarse_step, _fine_step) = midas_chart::price_step_for(level.price);

    // -- Header --
    let header = row![
        text("Edit Level").size(11).color(Color::WHITE),
        Space::new().width(Fill),
        button(text("\u{00D7}").size(13)) // x close
            .on_press(Message::ChartCloseLevelEditor(chart_id))
            .padding([0, 4])
            .style(button::text),
    ]
    .align_y(iced::Alignment::Center)
    .spacing(4);

    // -- Price input with up/down --
    let price_input_field = text_input("Price", price_input)
        .on_input(move |s| Message::LevelEditorPriceChanged(chart_id, level_id, s))
        .size(11)
        .width(100);

    let price_up = button(text("\u{25B2}").size(8)) // upward triangle
        .on_press(Message::LevelEditorPriceStep(
            chart_id,
            level_id,
            coarse_step,
        ))
        .padding([2, 4])
        .style(button::text);

    let price_down = button(text("\u{25BC}").size(8)) // downward triangle
        .on_press(Message::LevelEditorPriceStep(
            chart_id,
            level_id,
            -coarse_step,
        ))
        .padding([2, 4])
        .style(button::text);

    let price_row_inner = row![
        text("Price")
            .size(10)
            .color(Color::from_rgba(0.6, 0.6, 0.65, 1.0)),
        price_input_field,
        column![price_up, price_down].spacing(0),
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    // Wrap in mouse_area to capture scroll wheel for price adjustment.
    let price_row = iced::widget::mouse_area(price_row_inner).on_scroll(
        move |delta| {
            let lines = match delta {
                iced::mouse::ScrollDelta::Lines { y, .. } => y,
                iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
            };
            Message::LevelEditorPriceStep(chart_id, level_id, coarse_step * lines as f64)
        },
    );

    // -- Label input --
    let current_label = level.label.as_deref().unwrap_or("");
    let label_input = text_input("Label", current_label)
        .on_input(move |s| Message::LevelEditorLabelChanged(chart_id, level_id, s))
        .size(11)
        .width(140);

    let label_row = row![
        text("Label")
            .size(10)
            .color(Color::from_rgba(0.6, 0.6, 0.65, 1.0)),
        label_input,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    // -- Color presets --
    let color_presets: [[f32; 4]; 8] = [
        [0.22, 0.55, 0.95, 0.8], // blue
        [0.95, 0.22, 0.22, 0.8], // red
        [0.22, 0.85, 0.35, 0.8], // green
        [1.0, 0.843, 0.0, 1.0],  // gold
        [0.95, 0.55, 0.15, 0.9], // orange
        [0.7, 0.35, 0.95, 0.8],  // purple
        [0.0, 0.85, 0.85, 0.8],  // cyan
        [0.85, 0.85, 0.85, 0.8], // gray
    ];

    let mut color_buttons = Row::new().spacing(3);
    for preset in &color_presets {
        let c = *preset;
        let is_selected = (level.color[0] - c[0]).abs() < 0.05
            && (level.color[1] - c[1]).abs() < 0.05
            && (level.color[2] - c[2]).abs() < 0.05;
        let border_color = if is_selected {
            Color::WHITE
        } else {
            Color::TRANSPARENT
        };
        let swatch_color = Color::from_rgba(c[0], c[1], c[2], c[3]);
        color_buttons = color_buttons.push(
            button(Space::new().width(14).height(14))
                .on_press(Message::LevelEditorColorChanged(chart_id, level_id, c))
                .padding(0)
                .style(move |_theme: &iced::Theme, _status| button::Style {
                    background: Some(iced::Background::Color(swatch_color)),
                    border: iced::Border {
                        color: border_color,
                        width: if is_selected { 2.0 } else { 1.0 },
                        radius: 2.0.into(),
                    },
                    text_color: Color::WHITE,
                    ..Default::default()
                }),
        );
    }

    let color_row = row![
        text("Color")
            .size(10)
            .color(Color::from_rgba(0.6, 0.6, 0.65, 1.0)),
        color_buttons,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    // -- Thickness --
    let thicknesses: [f32; 4] = [1.0, 2.0, 3.0, 4.0];
    let mut thickness_buttons = Row::new().spacing(3);
    for &t in &thicknesses {
        let is_sel = (level.line_width - t).abs() < 0.1;
        let label = format!("{}px", t as u32);
        thickness_buttons = thickness_buttons.push(
            button(text(label).size(9))
                .on_press(Message::LevelEditorThicknessChanged(
                    chart_id, level_id, t,
                ))
                .padding([2, 6])
                .style(move |theme: &iced::Theme, status| {
                    let mut s = button::text(theme, status);
                    if is_sel {
                        s.background = Some(iced::Background::Color(Color::from_rgba(
                            0.3, 0.4, 0.6, 0.8,
                        )));
                        s.border.radius = 3.0.into();
                    }
                    s
                }),
        );
    }

    let thickness_row = row![
        text("Width")
            .size(10)
            .color(Color::from_rgba(0.6, 0.6, 0.65, 1.0)),
        thickness_buttons,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    // -- Icon selector --
    let mut icon_buttons = Row::new().spacing(3);
    for icon_variant in midas_chart::LevelIcon::all() {
        let is_sel = level.icon == *icon_variant;
        let label = match icon_variant.as_char() {
            Some(ch) => ch.to_string(),
            None => "\u{2014}".to_string(), // em dash for "None"
        };
        let icon_clone = icon_variant.clone();
        icon_buttons = icon_buttons.push(
            button(text(label).size(20))
                .on_press(Message::LevelEditorIconChanged(
                    chart_id, level_id, icon_clone,
                ))
                .padding([2, 5])
                .style(move |theme: &iced::Theme, status| {
                    let mut s = button::text(theme, status);
                    if is_sel {
                        s.background = Some(iced::Background::Color(Color::from_rgba(
                            0.3, 0.4, 0.6, 0.8,
                        )));
                        s.border.radius = 3.0.into();
                    }
                    s
                }),
        );
    }

    let icon_row = row![
        text("Icon")
            .size(10)
            .color(Color::from_rgba(0.6, 0.6, 0.65, 1.0)),
        icon_buttons,
    ]
    .spacing(4)
    .align_y(iced::Alignment::Center);

    // -- Lock toggle + Delete --
    let lock_label = if level.locked { "Lock" } else { "Unlock" };
    let is_locked = level.locked;
    let lock_btn = button(text(lock_label).size(10))
        .on_press(Message::LevelEditorToggleLock(chart_id, level_id))
        .padding([3, 8])
        .style(move |theme: &iced::Theme, status| {
            let mut s = button::text(theme, status);
            if is_locked {
                s.background = Some(iced::Background::Color(Color::from_rgba(
                    0.5, 0.35, 0.1, 0.6,
                )));
                s.border.radius = 3.0.into();
            }
            s
        });

    let delete_btn = button(
        text("Delete")
            .size(10)
            .color(Color::from_rgba(1.0, 0.4, 0.4, 1.0)),
    )
    .on_press(Message::ChartDeleteLevel(chart_id, level_id))
    .padding([3, 8])
    .style(button::text);

    let action_row = row![lock_btn, Space::new().width(Fill), delete_btn]
        .spacing(4)
        .align_y(iced::Alignment::Center);

    // -- Divider helper (styled thin container instead of rule widget) --
    let divider = || -> Element<'a, Message> {
        container(Space::new().width(Fill).height(1))
            .width(Fill)
            .style(|_theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.3, 0.35, 0.45, 0.5,
                ))),
                ..Default::default()
            })
            .into()
    };

    // -- Assemble popup --
    let popup_content = column![
        header,
        divider(),
        price_row,
        label_row,
        color_row,
        thickness_row,
        icon_row,
        divider(),
        action_row,
    ]
    .spacing(6)
    .padding(10)
    .width(240);

    let popup = container(popup_content).style(|_theme: &iced::Theme| container::Style {
        background: Some(iced::Background::Color(Color::from_rgba(
            0.10, 0.12, 0.16, 0.95,
        ))),
        border: iced::Border {
            color: Color::from_rgba(0.3, 0.35, 0.45, 0.7),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(2.0, 4.0),
            blur_radius: 12.0,
        },
        ..Default::default()
    });

    // Position: clamp to viewport bounds.
    let popup_w: f32 = 240.0;
    let popup_h: f32 = 280.0;
    let left = (screen_pos.0 + 10.0)
        .min((viewport_width as f32) - popup_w - 10.0)
        .max(0.0);
    let top = (screen_pos.1 - popup_h / 2.0)
        .min((viewport_height as f32) - popup_h - 10.0)
        .max(0.0);

    container(popup)
        .padding(iced::Padding::ZERO.top(top).left(left))
        .width(Fill)
        .height(Fill)
        .into()
}

/// Compute `LevelRender` data from a chart panel's state for use in overlays.
fn compute_level_renders(chart: &ChartPanel) -> Vec<midas_chart::LevelRender> {
    let cam = &chart.chart_state.camera;
    chart
        .chart_state
        .levels
        .iter()
        .map(|lev| midas_chart::LevelRender {
            id: lev.id,
            price: lev.price,
            screen_y: cam.price_to_y(lev.price),
            color: lev.color,
            line_width: lev.line_width,
            is_selected: chart.chart_state.selected_level == Some(lev.id),
            is_being_dragged: false,
            original_screen_y: None,
            label_text: midas_chart::format_price(lev.price),
            label: lev.label.clone(),
            icon: lev.icon.clone(),
            locked: lev.locked,
        })
        .collect()
}

// ── Button style helpers ────────────────────────────────────────────

/// Button style: muted text by default, white text + subtle bg on hover.
fn hover_text_button_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
    let text_color = match status {
        button::Status::Hovered | button::Status::Pressed => Color::WHITE,
        _ => theme::TEXT_MUTED,
    };
    let background = match status {
        button::Status::Hovered => Some(iced::Background::Color(Color::from_rgba(
            1.0, 1.0, 1.0, 0.1,
        ))),
        button::Status::Pressed => Some(iced::Background::Color(Color::from_rgba(
            1.0, 1.0, 1.0, 0.15,
        ))),
        _ => None,
    };
    button::Style {
        text_color,
        background,
        ..Default::default()
    }
}
