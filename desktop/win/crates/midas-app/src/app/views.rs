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

            let chart_area = stack![shader, date_overlay, price_overlay].width(Fill).height(Fill);

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

            return container(stack![shader, date_overlay, price_overlay].width(Fill).height(Fill))
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
