//! View functions for the main application.
//!
//! Builds the widget tree: toolbar, pane grid, title bars, chart body,
//! status bar, and floating chart windows.

use std::sync::Arc;

use iced::widget::pane_grid::{self, PaneGrid};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, stack, text, text_input, Column, Row,
    Space,
};
use iced::{window, Color, Element, Fill, Length};

use midas_chart::AnnotationId;
use midas_core::{ChartId, LinkColor, LinkMode, Timeframe, WatchlistId};

// Order panel overlay positioning
const ORDER_PANEL_TOP_PADDING: f32 = 60.0;
const ORDER_PANEL_RIGHT_PADDING: f32 = 20.0;

use crate::layout::PanelContent;
use crate::link::{link_color_rgba, link_mode_indicator_rgba, LinkDimension, PickerTarget};
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
            return self.view_floating_chart(window_id, chart);
        }

        // Main window (or fallback for unknown windows).
        let toolbar = self.view_toolbar();
        let content = self.view_content();
        let status_bar = self.view_status_bar();

        // Drag overlay: floating label near cursor when dragging a ticker.
        if let Some(ref drag) = self.dragging_ticker {
            let label = container(
                text(drag.symbol.clone())
                    .size(13)
                    .color(Color::WHITE),
            )
            .padding([4, 8])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.15, 0.35, 0.65, 0.92,
                ))),
                border: iced::Border {
                    color: Color::from_rgb(0.3, 0.5, 0.8),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });

            // Position the label offset from the current cursor.
            let pos = self.cursor_position;
            let drag_preview = container(label)
                .width(Length::Shrink)
                .height(Length::Shrink)
                .padding(iced::Padding::ZERO
                    .top(pos.y + 16.0)
                    .left(pos.x + 12.0));

            let base = column![toolbar, content, status_bar];
            return stack![base, drag_preview].into();
        }

        // Order panel overlay: floats on top of the main layout.
        if self.order_panel.visible {
            let panel_widget = self.view_order_panel();
            let positioned = container(panel_widget)
                .width(Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .padding(iced::Padding {
                    top: ORDER_PANEL_TOP_PADDING,
                    right: ORDER_PANEL_RIGHT_PADDING,
                    bottom: 0.0,
                    left: 0.0,
                });

            return stack![
                column![toolbar, content, status_bar],
                positioned,
            ]
            .into();
        }

        column![toolbar, content, status_bar].into()
    }

    /// Build the view for a floating (pop-out) chart window.
    fn view_floating_chart<'a>(
        &'a self,
        wid: window::Id,
        chart: &'a ChartPanel,
    ) -> Element<'a, Message> {
        // If data is loaded, render via GPU Shader widget.
        if let (LoadState::Loaded, Some(ref data)) = (&chart.load_state, &chart.data) {
            let snapshot = crate::chart_widget::ChartRenderSnapshot {
                symbol: chart.symbol.clone(),
                data: Some(Arc::clone(data)),
                camera: chart.chart_state.camera.clone(),
                dirty: chart.chart_state.dirty.clone(),
                crosshair_pos: chart.chart_state.crosshair.render_pos(),
                levels: self.level_store.levels_for(&chart.symbol).to_vec(),
                viewport_width: chart.chart_state.camera.viewport_width,
                viewport_height: chart.chart_state.camera.viewport_height,
                collapse_gaps: chart.chart_state.collapse_gaps,
                timeline_border_ratio: chart.chart_state.timeline_border_ratio,
                volume_scale: chart.chart_state.volume_scale,
                show_volume_profile: chart.chart_state.show_volume_profile,
                show_levels: chart.chart_state.show_levels,
                data_time_start: chart.chart_state.data_time_start,
                data_time_end: chart.chart_state.data_time_end,
                editing_level_id: chart.editing_level_id,
                level_tool: chart.chart_state.level_tool.clone(),
                level_placing: self.level_placing,
                ghost_crosshair: compute_ghost_crosshair(
                    &self.crosshair_sync,
                    ChartId::new(0),
                    &chart.symbol,
                    &chart.chart_state,
                    chart.data.as_deref(),
                ),
                ghost_preview_price: self.placing_preview.as_ref().and_then(
                    |(src_id, sym, price)| {
                        if *src_id != ChartId::new(0) && chart.symbol == *sym {
                            Some(*price)
                        } else {
                            None
                        }
                    },
                ),
                placing_cursor_chart: self.placing_preview.as_ref().map(|(id, _, _)| *id),
                bracket_annotations: self.annotation_store.get(&chart.symbol)
                    .iter()
                    .filter(|a| matches!(
                        a.kind,
                        midas_chart::widget::AnnotationKind::OrderBracket(_)
                    ))
                    .cloned()
                    .collect(),
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

            let price_overlay =
                build_price_label_overlay(camera, chart.chart_state.timeline_border_ratio);

            // Build level-related overlays for floating window.
            let floating_chart_id = ChartId::new(0);
            let drawing_panel = build_drawing_panel(floating_chart_id, self.level_placing);

            // Gerchik ATR overlay — reads from the central market_cache
            // (computed from D1 bars), not from intraday aggregation.
            let gerchik_atr = gatr_render_from_cache(&self.market_cache, &chart.symbol);

            let mut chart_layers: Vec<Element<'_, Message>> =
                vec![shader.into(), date_overlay, price_overlay];

            chart_layers.push(build_gerchik_atr_overlay(gerchik_atr.as_ref()));

            let store_levels = self.level_store.levels_for(&chart.symbol);
            if chart.chart_state.show_levels {
                let level_renders = compute_level_renders(store_levels, chart);
                chart_layers.push(build_level_labels_overlay(
                    &level_renders,
                    chart.chart_state.camera.viewport_height,
                ));
            }

            // Crosshair axis labels for floating window.
            let crosshair_labels = midas_chart::compute_crosshair_labels(
                chart.chart_state.crosshair.render_pos(),
                camera,
                data.as_ref(),
                chart.chart_state.collapse_gaps,
            );
            chart_layers.push(build_crosshair_label_overlay(
                crosshair_labels.as_ref(),
                chart.chart_state.timeline_border_ratio,
                chart.chart_state.camera.viewport_width,
                chart.chart_state.camera.viewport_height,
            ));

            chart_layers.push(drawing_panel);

            // Level editor popup (when a level is being edited).
            if let (Some(editing_id), Some(screen_pos)) =
                (chart.editing_level_id, chart.editing_level_screen_pos)
            {
                if let Some(level) = store_levels.iter().find(|l| l.id == editing_id) {
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

            // Link color picker overlay (when open for this floating chart).
            if let Some((PickerTarget::Floating(picker_wid), dim)) = self.link_picker_open {
                if picker_wid == wid {
                    // Backdrop to dismiss picker on click outside.
                    chart_layers.push(
                        iced::widget::mouse_area(
                            Space::new().width(Fill).height(Fill),
                        )
                        .on_press(Message::DismissLinkPicker)
                        .into(),
                    );
                    let picker = self.build_link_picker(dim, move |mode| match dim {
                        LinkDimension::Symbol => Message::FloatingSetSymbolLink(wid, mode),
                        LinkDimension::Timeframe => {
                            Message::FloatingSetTimeframeLink(wid, mode)
                        }
                    });
                    chart_layers.push(
                        container(picker)
                            .align_x(iced::alignment::Horizontal::Right)
                            .align_y(iced::alignment::Vertical::Top)
                            .padding([4, 4])
                            .width(Fill)
                            .height(Fill)
                            .into(),
                    );
                }
            }

            let chart_area = stack(chart_layers).width(Fill).height(Fill);

            // Symbol link button for floating chart.
            let bold_font = iced::Font {
                weight: iced::font::Weight::Bold,
                ..iced::Font::default()
            };
            let sym_link = chart.symbol_link;
            let sym_color = link_mode_indicator_rgba(sym_link);
            let float_s_btn = button(
                text("S").size(10).color(Color::WHITE).font(bold_font),
            )
            .on_press(Message::ToggleLinkPicker(
                PickerTarget::Floating(wid),
                LinkDimension::Symbol,
            ))
            .padding([2, 5])
            .style(move |_theme, _status| button::Style {
                background: Some(
                    Color::from_rgba(
                        sym_color[0],
                        sym_color[1],
                        sym_color[2],
                        sym_color[3],
                    )
                    .into(),
                ),
                text_color: Color::WHITE,
                border: iced::Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

            // Timeframe link button for floating chart.
            let tf_link = chart.timeframe_link;
            let tf_color = link_mode_indicator_rgba(tf_link);
            let float_t_btn = button(
                text("T").size(10).color(Color::WHITE).font(bold_font),
            )
            .on_press(Message::ToggleLinkPicker(
                PickerTarget::Floating(wid),
                LinkDimension::Timeframe,
            ))
            .padding([2, 5])
            .style(move |_theme, _status| button::Style {
                background: Some(
                    Color::from_rgba(
                        tf_color[0],
                        tf_color[1],
                        tf_color[2],
                        tf_color[3],
                    )
                    .into(),
                ),
                text_color: Color::WHITE,
                border: iced::Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

            // Header bar with symbol, link buttons, and timeframe.
            let header = container(
                row![
                    float_s_btn,
                    text(&chart.symbol).size(13).color(Color::WHITE),
                    float_t_btn,
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

        let wl_btn = button(text("Watchlist").size(12))
            .on_press(Message::AddWatchlist)
            .padding([4, 10])
            .style(hover_text_button_style);

        // Provider dropdowns (pushed to the right).
        let data_names = self.providers.data_provider_names();
        let active_data = self.providers.active_data_provider_name();
        let data_picker = pick_list(data_names, Some(active_data), Message::DataProviderSelected)
            .text_size(11)
            .padding([3, 6])
            .style(dark_pick_list_style);

        let broker_names = self.providers.order_broker_names();
        let active_broker = self.providers.active_broker_display_name();
        let broker_picker =
            pick_list(broker_names, Some(active_broker), Message::OrderBrokerSelected)
                .text_size(11)
                .padding([3, 6])
                .style(dark_pick_list_style);

        let toolbar_row = row![
            layout_buttons,
            split_buttons,
            add_btn,
            wl_btn,
            Space::new().width(Fill),
            text("Data:").size(11).color(theme::TEXT_SECONDARY),
            data_picker,
            text("Broker:").size(11).color(theme::TEXT_SECONDARY),
            broker_picker,
        ]
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

                let (title_bar, body) = match pane_state.content {
                    PanelContent::Chart(chart_id) => {
                        let tb = self.view_pane_title_bar(chart_id, pane, pane_count);
                        let bd = self.view_pane_body(chart_id);
                        (tb, bd)
                    }
                    PanelContent::Watchlist(wl_id) => {
                        let tb = self.view_watchlist_title_bar(wl_id, pane);
                        let bd = self.view_watchlist_body(wl_id);
                        (tb, bd)
                    }
                };

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
            .on_click(Message::PaneFocused)
            .on_resize(6, Message::PaneResized)
            // Note: on_click fires PaneFocused for pane selection.
            // Drag-drop uses DragMouseUp with global hit-testing instead.
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
        let controls_row = self.view_title_bar_controls(chart_id, pane, pane_count);

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

        let levels_active = chart.map(|c| c.chart_state.show_levels).unwrap_or(true);
        let levels_btn = if levels_active {
            button(text("LV").size(10).color(Color::WHITE))
                .on_press(Message::ToggleLevels(chart_id))
                .padding([1, 4])
                .style(button::primary)
        } else {
            button(text("LV").size(10))
                .on_press(Message::ToggleLevels(chart_id))
                .padding([1, 4])
                .style(button::text)
        };

        let reset_btn = button(text("R").size(10))
            .on_press(Message::ResetChart(chart_id))
            .padding([1, 4])
            .style(button::text);

        row![
            ticker_input,
            tf_row,
            collapse_btn,
            vp_btn,
            levels_btn,
            reset_btn,
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .height(24)
        .into()
    }

    /// Build the controls (right) area of a pane's TitleBar.
    ///
    /// Layout: `[S][T]  [⧉][×]`
    fn view_title_bar_controls(
        &self,
        chart_id: ChartId,
        pane: pane_grid::Pane,
        pane_count: usize,
    ) -> Element<'_, Message> {
        let chart = self.charts.get(&chart_id);

        // Symbol link button.
        let bold_font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let sym_link = chart.map(|c| c.symbol_link).unwrap_or(LinkMode::Unlinked);
        let sym_color = link_mode_indicator_rgba(sym_link);
        let s_btn = button(
            text("S").size(10).color(Color::WHITE).font(bold_font),
        )
        .on_press(Message::ToggleLinkPicker(
            PickerTarget::Docked(chart_id),
            LinkDimension::Symbol,
        ))
        .padding([2, 5])
        .style(move |_theme, _status| button::Style {
            background: Some(
                Color::from_rgba(sym_color[0], sym_color[1], sym_color[2], sym_color[3])
                    .into(),
            ),
            text_color: Color::WHITE,
            border: iced::Border {
                radius: 2.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

        // Timeframe link button.
        let tf_link = chart.map(|c| c.timeframe_link).unwrap_or(LinkMode::Unlinked);
        let tf_color = link_mode_indicator_rgba(tf_link);
        let t_btn = button(
            text("T").size(10).color(Color::WHITE).font(bold_font),
        )
        .on_press(Message::ToggleLinkPicker(
            PickerTarget::Docked(chart_id),
            LinkDimension::Timeframe,
        ))
        .padding([2, 5])
        .style(move |_theme, _status| button::Style {
            background: Some(
                Color::from_rgba(tf_color[0], tf_color[1], tf_color[2], tf_color[3])
                    .into(),
            ),
            text_color: Color::WHITE,
            border: iced::Border {
                radius: 2.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

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

        row![s_btn, t_btn, Space::new().width(4), pop_out_btn, close_btn]
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
                crosshair_pos: chart.chart_state.crosshair.render_pos(),
                levels: self.level_store.levels_for(&chart.symbol).to_vec(),
                viewport_width: chart.chart_state.camera.viewport_width,
                viewport_height: chart.chart_state.camera.viewport_height,
                collapse_gaps: chart.chart_state.collapse_gaps,
                timeline_border_ratio: chart.chart_state.timeline_border_ratio,
                volume_scale: chart.chart_state.volume_scale,
                show_volume_profile: chart.chart_state.show_volume_profile,
                show_levels: chart.chart_state.show_levels,
                data_time_start: chart.chart_state.data_time_start,
                data_time_end: chart.chart_state.data_time_end,
                editing_level_id: chart.editing_level_id,
                level_tool: chart.chart_state.level_tool.clone(),
                level_placing: self.level_placing,
                ghost_crosshair: compute_ghost_crosshair(
                    &self.crosshair_sync,
                    chart_id,
                    &chart.symbol,
                    &chart.chart_state,
                    chart.data.as_deref(),
                ),
                ghost_preview_price: self.placing_preview.as_ref().and_then(
                    |(src_id, sym, price)| {
                        if *src_id != chart_id && chart.symbol == *sym {
                            Some(*price)
                        } else {
                            None
                        }
                    },
                ),
                placing_cursor_chart: self.placing_preview.as_ref().map(|(id, _, _)| *id),
                bracket_annotations: self.annotation_store.get(&chart.symbol)
                    .iter()
                    .filter(|a| matches!(
                        a.kind,
                        midas_chart::widget::AnnotationKind::OrderBracket(_)
                    ))
                    .cloned()
                    .collect(),
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

            let price_overlay =
                build_price_label_overlay(camera, chart.chart_state.timeline_border_ratio);

            // Build level-related overlays.
            let drawing_panel = build_drawing_panel(chart_id, self.level_placing);

            // Gerchik ATR overlay — reads from the central market_cache
            // (computed from D1 bars), not from intraday aggregation.
            let gerchik_atr = gatr_render_from_cache(&self.market_cache, &chart.symbol);

            let mut chart_layers: Vec<Element<'_, Message>> =
                vec![shader.into(), date_overlay, price_overlay];

            chart_layers.push(build_gerchik_atr_overlay(gerchik_atr.as_ref()));

            let store_levels = self.level_store.levels_for(&chart.symbol);
            if chart.chart_state.show_levels {
                let level_renders = compute_level_renders(store_levels, chart);
                chart_layers.push(build_level_labels_overlay(
                    &level_renders,
                    chart.chart_state.camera.viewport_height,
                ));
            }

            // Crosshair axis labels (white badges at arm endpoints).
            let crosshair_labels = midas_chart::compute_crosshair_labels(
                chart.chart_state.crosshair.render_pos(),
                camera,
                data.as_ref(),
                chart.chart_state.collapse_gaps,
            );
            chart_layers.push(build_crosshair_label_overlay(
                crosshair_labels.as_ref(),
                chart.chart_state.timeline_border_ratio,
                chart.chart_state.camera.viewport_width,
                chart.chart_state.camera.viewport_height,
            ));

            chart_layers.push(drawing_panel);

            // Level editor popup (when a level is being edited).
            if let (Some(editing_id), Some(screen_pos)) =
                (chart.editing_level_id, chart.editing_level_screen_pos)
            {
                if let Some(level) = store_levels.iter().find(|l| l.id == editing_id) {
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

            // Link color picker overlay (when open for this chart).
            if let Some((PickerTarget::Docked(picker_id), dim)) = self.link_picker_open {
                if picker_id == chart_id {
                    // Backdrop to dismiss picker on click outside.
                    chart_layers.push(
                        iced::widget::mouse_area(
                            Space::new().width(Fill).height(Fill),
                        )
                        .on_press(Message::DismissLinkPicker)
                        .into(),
                    );
                    let picker = self.build_link_picker(dim, move |mode| match dim {
                        LinkDimension::Symbol => Message::SetSymbolLink(chart_id, mode),
                        LinkDimension::Timeframe => {
                            Message::SetTimeframeLink(chart_id, mode)
                        }
                    });
                    chart_layers.push(
                        container(picker)
                            .align_x(iced::alignment::Horizontal::Right)
                            .align_y(iced::alignment::Vertical::Top)
                            .padding([4, 4])
                            .width(Fill)
                            .height(Fill)
                            .into(),
                    );
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

// ── Link picker ────────────────────────────────────────────────────

impl MidasApp {
    /// Build the link color picker dropdown overlay.
    ///
    /// Shows 8 color options, a "Listen for any changes" option, and
    /// a "Not Linked" option. The `msg_builder` closure creates the
    /// appropriate `Message` for each option.
    fn build_link_picker(
        &self,
        dimension: LinkDimension,
        msg_builder: impl Fn(LinkMode) -> Message,
    ) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::with_capacity(10);

        // 8 color options.
        for color in LinkColor::ALL {
            let mode = LinkMode::Color(color);
            let rgba = link_color_rgba(color);
            let label = color.display_name();
            let msg = msg_builder(mode);

            let color_swatch = container(Space::new().width(12).height(12)).style(
                move |_| container::Style {
                    background: Some(
                        Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).into(),
                    ),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );

            items.push(
                button(
                    row![color_swatch, text(label).size(11)]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                )
                .on_press(msg)
                .padding([3, 8])
                .width(Fill)
                .style(button::text)
                .into(),
            );
        }

        // "Listen for any changes" option.
        let listen_msg = msg_builder(LinkMode::ListenAll);
        let listen_rgba = link_mode_indicator_rgba(LinkMode::ListenAll);
        let listen_swatch = container(Space::new().width(12).height(12)).style(
            move |_| container::Style {
                background: Some(
                    Color::from_rgba(
                        listen_rgba[0],
                        listen_rgba[1],
                        listen_rgba[2],
                        listen_rgba[3],
                    )
                    .into(),
                ),
                border: iced::Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        items.push(
            button(
                row![
                    listen_swatch,
                    text("Listen *").size(11)
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center),
            )
            .on_press(listen_msg)
            .padding([3, 8])
            .width(Fill)
            .style(button::text)
            .into(),
        );

        // "Not Linked" option.
        let unlinked_msg = msg_builder(LinkMode::Unlinked);
        let gray_rgba = link_mode_indicator_rgba(LinkMode::Unlinked);
        let gray_swatch = container(Space::new().width(12).height(12)).style(
            move |_| container::Style {
                background: Some(
                    Color::from_rgba(gray_rgba[0], gray_rgba[1], gray_rgba[2], gray_rgba[3])
                        .into(),
                ),
                border: iced::Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        items.push(
            button(
                row![gray_swatch, text("Not Linked").size(11)]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
            )
            .on_press(unlinked_msg)
            .padding([3, 8])
            .width(Fill)
            .style(button::text)
            .into(),
        );

        container(column(items).spacing(1).width(130))
            .style(|_| container::Style {
                background: Some(Color::from_rgb(0.15, 0.15, 0.18).into()),
                border: iced::Border {
                    color: Color::from_rgb(0.3, 0.3, 0.35),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
            .padding(4)
            .into()
    }
}

// ── Watchlist ──────────────────────────────────────────────────────

impl MidasApp {
    /// Build the TitleBar for a watchlist pane.
    fn view_watchlist_title_bar(
        &self,
        wl_id: WatchlistId,
        pane: pane_grid::Pane,
    ) -> pane_grid::TitleBar<'_, Message> {
        let wl_link = self
            .watchlists
            .get(&wl_id)
            .map(|wl| wl.symbol_link)
            .unwrap_or(LinkMode::Unlinked);
        let wl_color = link_mode_indicator_rgba(wl_link);
        let bold_font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let wl_s_btn: Element<'_, Message> = button(
            text("S").size(10).color(Color::WHITE).font(bold_font),
        )
        .on_press(Message::ToggleLinkPicker(
            PickerTarget::Watchlist(wl_id),
            LinkDimension::Symbol,
        ))
        .padding([2, 5])
        .style(move |_theme, _status| button::Style {
            background: Some(
                Color::from_rgba(wl_color[0], wl_color[1], wl_color[2], wl_color[3])
                    .into(),
            ),
            text_color: Color::WHITE,
            border: iced::Border {
                radius: 2.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into();

        let close_btn: Element<'_, Message> = button(text("X").size(10))
            .on_press(Message::PaneClose(pane))
            .padding([2, 6])
            .style(hover_text_button_style)
            .into();

        pane_grid::TitleBar::new(
            row![text("Watchlist").size(14), Space::new().width(Fill)]
                .align_y(iced::Alignment::Center),
        )
        .controls(
            Element::from(
                row![wl_s_btn, Space::new().width(4), close_btn]
                    .spacing(2)
                    .align_y(iced::Alignment::Center),
            ),
        )
        .padding([2, 4])
        .always_show_controls()
        .style(|_theme| container::Style::default())
    }

    /// Build the body of a watchlist panel.
    fn view_watchlist_body(&self, wl_id: WatchlistId) -> Element<'_, Message> {
        let wl = match self.watchlists.get(&wl_id) {
            Some(wl) => wl,
            None => {
                return container(text("Watchlist not found").size(14))
                    .center_x(Fill)
                    .center_y(Fill)
                    .into();
            }
        };

        // Build WatchlistRow structs from tickers + cached market data.
        let empty_snapshot = midas_core::MarketSnapshot::default();
        let mut grid_rows: Vec<crate::watchlist_columns::WatchlistRow> = wl
            .tickers
            .iter()
            .map(|ticker| {
                let snap = self
                    .market_cache
                    .get(&ticker.symbol)
                    .unwrap_or(&empty_snapshot);
                let price_text = snap
                    .last_price
                    .map(|p| format!("{p:.2}"))
                    .unwrap_or_else(|| "--".into());
                let change_text = snap
                    .change_pct
                    .map(|c| format!("{c:+.2}%"))
                    .unwrap_or_else(|| "--".into());
                let change_color = match snap.change_pct {
                    Some(c) if c > 0.0 => Color::from_rgb(0.2, 0.8, 0.3),
                    Some(c) if c < 0.0 => Color::from_rgb(0.9, 0.25, 0.2),
                    _ => Color::from_rgb(0.6, 0.6, 0.6),
                };
                let gatr_text = snap
                    .gatr_pct
                    .map(|pct| format!("G.ATR {:.0}%", pct))
                    .unwrap_or_else(|| "--".into());
                let gatr_color = snap
                    .gatr_pct
                    .map(|_| {
                        let price_up = snap.change_pct.map_or(true, |c| c >= 0.0);
                        let c = midas_core::gatr_color(price_up);
                        Color::from_rgba(c[0], c[1], c[2], c[3])
                    })
                    .unwrap_or(Color::from_rgb(0.6, 0.6, 0.6));
                crate::watchlist_columns::WatchlistRow {
                    symbol: ticker.symbol.clone(),
                    favorite: ticker.favorite,
                    price_text,
                    change_text,
                    change_color,
                    gatr_text,
                    gatr_color,
                    wl_id,
                    price_value: snap.last_price,
                    change_value: snap.change_pct,
                }
            })
            .collect();

        // Sort: favorites first, then by grid sort spec.
        grid_rows.sort_by(|a, b| {
            let fav = b.favorite.cmp(&a.favorite);
            if fav != std::cmp::Ordering::Equal {
                return fav;
            }
            if let Some(sort) = &wl.grid_state.sort {
                let columns = crate::watchlist_columns::WatchlistColumn::all();
                if let Some(col) = columns.iter().find(|c| {
                    use midas_grid::GridColumn;
                    c.id() == sort.column_id
                }) {
                    use midas_grid::GridColumn;
                    let ord = col.compare(a, b);
                    return match sort.direction {
                        midas_grid::SortDirection::Ascending => ord,
                        midas_grid::SortDirection::Descending => ord.reverse(),
                    };
                }
            }
            std::cmp::Ordering::Equal
        });

        // Update selection to match selected_symbol (bridge index-based selection).
        // Find the index of the selected symbol in the sorted rows.
        let selected_idx = wl.selected_symbol.as_ref().and_then(|sym| {
            grid_rows.iter().position(|r| r.symbol == *sym)
        });

        // Build a temporary GridState copy with the correct selection index.
        let mut view_state = wl.grid_state.clone();
        if let Some(idx) = selected_idx {
            view_state.selection.select(idx);
        } else {
            view_state.selection.clear();
        }

        // Build grid header + body inline.
        // (The Grid builder can't be used here because columns/rows are local
        // variables whose borrows can't escape the function. The Grid API works
        // when data lives on &self — see Phase 2.)
        use crate::watchlist::{
            COL_CHANGE, COL_DELETE, COL_DRAG, COL_FAV, COL_GATR, COL_PRICE, COL_TICKER,
        };

        // Column definitions: (id, header_label, sortable, width).
        let col_defs: [(midas_grid::ColumnId, &str, bool); 7] = [
            (COL_DRAG, "", false),
            (COL_FAV, "\u{2605}", false),
            (COL_TICKER, "Ticker", true),
            (COL_PRICE, "Price", true),
            (COL_CHANGE, "Chg%", true),
            (COL_GATR, "G.ATR", true),
            (COL_DELETE, "", false),
        ];

        // Header row.
        let mut header_cells: Vec<Element<'_, Message>> = Vec::with_capacity(13);
        for (i, &(col_id, label, sortable)) in col_defs.iter().enumerate() {
            let width = view_state.column_width(col_id);

            let header_content: Element<'_, Message> = if sortable {
                let sort_indicator = view_state
                    .sort
                    .filter(|s| s.column_id == col_id)
                    .map(|s| s.direction.indicator())
                    .unwrap_or("");

                let msg = Message::WatchlistGrid(
                    wl_id,
                    midas_grid::GridMessage::SortToggled(col_id),
                );
                iced::widget::mouse_area(
                    container(row![text(label).size(12), text(sort_indicator).size(12)])
                        .width(width)
                        .padding([2, 4])
                        .style(|_| container::Style {
                            border: iced::Border {
                                color: midas_grid::GRID_HEADER_BORDER_COLOR,
                                width: 1.0,
                                radius: 0.0.into(),
                            },
                            ..Default::default()
                        }),
                )
                .on_release(msg)
                .into()
            } else if label.is_empty() {
                container(Space::new())
                    .width(width)
                    .padding([2, 4])
                    .style(|_| container::Style {
                        border: iced::Border {
                            color: midas_grid::GRID_HEADER_BORDER_COLOR,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..Default::default()
                    })
                    .into()
            } else {
                container(text(label).size(12))
                    .width(width)
                    .padding([2, 4])
                    .style(|_| container::Style {
                        border: iced::Border {
                            color: midas_grid::GRID_HEADER_BORDER_COLOR,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..Default::default()
                    })
                    .into()
            };

            // Wrap header cell with a resize handle on the right edge.
            // The handle is layered on top via stack so it doesn't add width.
            if i < col_defs.len() - 1 {
                let col_idx = i;
                let resize_handle = container(
                    iced::widget::mouse_area(Space::new().width(4).height(26))
                        .interaction(iced::mouse::Interaction::ResizingHorizontally)
                        .on_press(Message::WatchlistColumnResizeStart(wl_id, col_idx, 0.0)),
                )
                .width(Fill)
                .align_x(iced::alignment::Horizontal::Right);

                header_cells.push(
                    stack![header_content, resize_handle]
                        .width(width)
                        .into(),
                );
            } else {
                header_cells.push(header_content);
            }
        }
        let header = Row::with_children(header_cells).padding([0, 4]);

        // Body rows.
        let mut body_rows = Column::new();
        if grid_rows.is_empty() {
            body_rows = body_rows.push(
                container(text("Add tickers to get started").size(13))
                    .padding(20)
                    .center_x(Fill),
            );
        } else {
            for (row_idx, row_data) in grid_rows.iter().enumerate() {
                let is_selected = view_state.selection.is_selected(row_idx);
                let row_bg = if is_selected {
                    Color::from_rgba(0.2, 0.35, 0.55, 0.6)
                } else {
                    Color::TRANSPARENT
                };

                // Build cells matching column order.
                let fav_label = if row_data.favorite { "\u{2605}" } else { "\u{2606}" };
                let sym = row_data.symbol.clone();
                let sym_del = row_data.symbol.clone();
                let sym_drag = row_data.symbol.clone();

                let fav_btn = button(text(fav_label).size(12))
                    .on_press(Message::WatchlistToggleFavorite(wl_id, sym))
                    .padding([2, 4])
                    .style(hover_text_button_style);

                let del_btn = button(text("\u{00D7}").size(12))
                    .on_press(Message::WatchlistRemoveTicker(wl_id, sym_del))
                    .padding([2, 4])
                    .style(hover_text_button_style);

                // Ticker cell is a drag handle — clicking it starts a drag.
                let ticker_cell = iced::widget::mouse_area(
                    text(row_data.symbol.clone())
                        .size(13)
                        .wrapping(iced::widget::text::Wrapping::None)
                        .color(theme::TEXT_PRIMARY),
                )
                .on_press(Message::WatchlistTickerPressed(wl_id, sym_drag));

                let w = |col_id| view_state.column_width(col_id);

                use iced::widget::text::Wrapping;
                let inner_row = Row::with_children(vec![
                    grid_data_cell(fav_btn.into(), w(COL_FAV)),
                    grid_data_cell(ticker_cell.into(), w(COL_TICKER)),
                    grid_data_cell(text(row_data.price_text.clone()).size(13).wrapping(Wrapping::None).color(theme::TEXT_PRIMARY).into(), w(COL_PRICE)),
                    grid_data_cell(
                        text(row_data.change_text.clone())
                            .size(13)
                            .wrapping(Wrapping::None)
                            .color(row_data.change_color)
                            .into(),
                        w(COL_CHANGE),
                    ),
                    grid_data_cell(
                        text(row_data.gatr_text.clone())
                            .size(13)
                            .wrapping(Wrapping::None)
                            .color(row_data.gatr_color)
                            .into(),
                        w(COL_GATR),
                    ),
                    grid_data_cell(del_btn.into(), w(COL_DELETE)),
                ])
                .padding([0, 4])
                .align_y(iced::Alignment::Center);

                // Emit WatchlistTickerSelected directly with the symbol.
                // This avoids the sorted-index mismatch: the view knows the
                // correct symbol at each visual row position.
                let sym_for_select = row_data.symbol.clone();
                let msg = Message::WatchlistTickerSelected(wl_id, sym_for_select);
                let ticker_row = iced::widget::mouse_area(
                    container(inner_row).style(move |_| container::Style {
                        background: Some(row_bg.into()),
                        ..Default::default()
                    }),
                )
                .on_release(msg);

                body_rows = body_rows.push(ticker_row);
            }
        }

        // Add ticker input row.
        let add_input = text_input("Add ticker...", &wl.add_ticker_input)
            .on_input(move |val| Message::WatchlistTickerInputChanged(wl_id, val))
            .on_submit(Message::WatchlistAddTicker(wl_id))
            .size(13)
            .width(200);

        let add_btn = button(text("Add").size(12))
            .on_press(Message::WatchlistAddTicker(wl_id))
            .padding([4, 8])
            .style(hover_text_button_style);

        let add_row = row![add_input, add_btn]
            .spacing(4)
            .padding([6, 8])
            .align_y(iced::Alignment::Center);

        let main_content: Element<'_, Message> = column![
            header,
            scrollable(body_rows).height(Fill),
            add_row,
        ]
        .width(Fill)
        .height(Fill)
        .into();

        // Wrap in stack only when overlays are needed (resize or link picker).
        let needs_resize_overlay = self
            .resizing_column
            .map(|(id, _, _, _)| id == wl_id)
            .unwrap_or(false);

        let needs_link_picker = matches!(
            self.link_picker_open,
            Some((PickerTarget::Watchlist(id), _)) if id == wl_id
        );

        if !needs_resize_overlay && !needs_link_picker {
            return main_content;
        }

        let mut body_layers: Vec<Element<'_, Message>> = vec![main_content];

        // Global resize overlay (when actively dragging a column divider).
        if needs_resize_overlay {
            body_layers.push(
                iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                    .interaction(iced::mouse::Interaction::ResizingHorizontally)
                    .on_move(|point| Message::WatchlistColumnResizing(point.x))
                    .on_release(Message::WatchlistColumnResizeEnd)
                    .into(),
            );
        }

        let body = stack(body_layers).width(Fill).height(Fill);

        // Link picker overlay.
        if let Some((PickerTarget::Watchlist(picker_wl_id), dim)) = self.link_picker_open {
            if picker_wl_id == wl_id {
                let backdrop = iced::widget::mouse_area(
                    Space::new().width(Fill).height(Fill),
                )
                .on_press(Message::DismissLinkPicker);

                let picker = self.build_link_picker(dim, move |mode| {
                    Message::WatchlistSetSymbolLink(wl_id, mode)
                });

                return stack![
                    body,
                    backdrop,
                    container(picker)
                        .align_x(iced::alignment::Horizontal::Right)
                        .align_y(iced::alignment::Vertical::Top)
                        .padding([4, 4])
                        .width(Fill)
                        .height(Fill)
                ]
                .width(Fill)
                .height(Fill)
                .into();
            }
        }

        body.into()
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

        // Connection indicator: green dot + provider name.
        let conn = self.connection_indicator();

        // Broker connection indicator: colored dot + broker name.
        let broker_indicator = {
            let (dot_color, label) = if self.broker_connection_display == "Ready" {
                let broker_name = self
                    .providers
                    .active_broker()
                    .map(|b| b.name().to_string())
                    .unwrap_or_else(|| "Broker".to_string());
                (Color::from_rgb(0.2, 0.8, 0.2), format!("Broker: {broker_name}"))
            } else if self.broker_connection_display == "Disconnected" {
                (
                    Color::from_rgb(0.6, 0.6, 0.6),
                    format!("Broker: {}", self.broker_connection_display),
                )
            } else {
                (
                    Color::from_rgb(0.9, 0.7, 0.2),
                    format!("Broker: {}", self.broker_connection_display),
                )
            };
            row![
                text("\u{25CF}").size(10).color(dot_color),
                text(format!(" {label}")).size(12).color(theme::TEXT_SECONDARY),
            ]
            .align_y(iced::Alignment::Center)
        };

        let status_row = row![
            conn,
            text(" | ").size(12).color(theme::TEXT_MUTED),
            broker_indicator,
            text(" | ").size(12).color(theme::TEXT_MUTED),
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

    /// Build a small connection status indicator for the status bar.
    ///
    /// Shows a colored dot and the active provider name.
    fn connection_indicator(&self) -> Element<'_, Message> {
        let provider_name = self.providers.active_data_provider_name();
        let is_connected = self
            .providers
            .active_data_provider()
            .map_or(false, |p| p.is_connected());
        let dot_color = if is_connected {
            Color::from_rgb(0.2, 0.8, 0.2) // green
        } else {
            Color::from_rgb(0.6, 0.6, 0.6) // grey
        };
        row![
            text("\u{25CF}").size(10).color(dot_color),
            text(format!(" {provider_name}"))
                .size(12)
                .color(theme::TEXT_SECONDARY),
        ]
        .align_y(iced::Alignment::Center)
        .into()
    }
}

// ── Order panel ────────────────────────────────────────────────────

impl MidasApp {
    /// Build the order panel overlay widget.
    fn view_order_panel(&self) -> Element<'_, Message> {
        let panel = &self.order_panel;

        // Side toggle buttons.
        let buy_style: fn(&iced::Theme, button::Status) -> button::Style =
            if panel.side == crate::order_panel::OrderSide::Buy {
                active_buy_button_style
            } else {
                inactive_side_button_style
            };
        let sell_style: fn(&iced::Theme, button::Status) -> button::Style =
            if panel.side == crate::order_panel::OrderSide::Sell {
                active_sell_button_style
            } else {
                inactive_side_button_style
            };

        let side_row = row![
            button(text("BUY").size(14))
                .on_press(Message::OrderPanelSetSide(
                    crate::order_panel::OrderSide::Buy,
                ))
                .padding([8, 20])
                .style(buy_style),
            button(text("SELL").size(14))
                .on_press(Message::OrderPanelSetSide(
                    crate::order_panel::OrderSide::Sell,
                ))
                .padding([8, 20])
                .style(sell_style),
            Space::new().width(Fill),
            text("Market")
                .size(12)
                .color(Color::from_rgb(0.6, 0.6, 0.6)),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        // Symbol and price display.
        let price_text = panel
            .last_price
            .map(|p| format!("Last: {p:.2}"))
            .unwrap_or_else(|| "Last: --".to_string());
        let symbol_row = row![
            text(format!("Symbol: {}", panel.symbol)).size(12),
            Space::new().width(Fill),
            text(price_text).size(12),
        ];

        // Quantity input.
        let qty_row = row![
            text("Qty:").size(12).width(40),
            text_input("100", &panel.quantity)
                .on_input(Message::OrderPanelSetQuantity)
                .size(12)
                .width(100),
            text("shares")
                .size(11)
                .color(Color::from_rgb(0.5, 0.5, 0.5)),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        // Take Profit section.
        let tp_section = {
            let mut col = Column::new().spacing(4);
            let tp_check = row![iced::widget::checkbox(
                panel.tp_enabled,
            )
            .label("Take Profit")
            .on_toggle(Message::OrderPanelToggleTp)
            .size(14),];
            col = col.push(tp_check);
            if panel.tp_enabled {
                let tp_input = row![
                    text("Price:").size(11).width(40),
                    text_input("0.00", &panel.tp_value)
                        .on_input(Message::OrderPanelSetTpValue)
                        .size(12)
                        .width(100),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);
                col = col.push(tp_input);
            }
            col
        };

        // Stop Loss section.
        let sl_section = {
            let mut col = Column::new().spacing(4);
            let sl_check = row![iced::widget::checkbox(
                panel.sl_enabled,
            )
            .label("Stop Loss")
            .on_toggle(Message::OrderPanelToggleSl)
            .size(14),];
            col = col.push(sl_check);
            if panel.sl_enabled {
                let sl_input = row![
                    text("Price:").size(11).width(40),
                    text_input("0.00", &panel.sl_value)
                        .on_input(Message::OrderPanelSetSlValue)
                        .size(12)
                        .width(100),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);
                col = col.push(sl_input);
            }
            col
        };

        // Risk/Reward display.
        let rr_row = if let Some(last) = panel.last_price {
            let tp_price = if panel.tp_enabled {
                panel.tp_value.parse::<f64>().ok().map(|val| {
                    crate::order_panel::resolve_price(
                        panel.tp_mode,
                        val,
                        last,
                        panel.side,
                        true,
                    )
                })
            } else {
                None
            };
            let sl_price = if panel.sl_enabled {
                panel.sl_value.parse::<f64>().ok().map(|val| {
                    crate::order_panel::resolve_price(
                        panel.sl_mode,
                        val,
                        last,
                        panel.side,
                        false,
                    )
                })
            } else {
                None
            };
            let qty = panel.quantity.parse::<f64>().unwrap_or(0.0);
            if let Some(rr) =
                crate::order_panel::calculate_risk_reward(last, tp_price, sl_price, qty)
            {
                row![
                    text(format!("Risk: ${:.0}", rr.total_risk))
                        .size(11)
                        .color(Color::from_rgb(0.9, 0.3, 0.3)),
                    Space::new().width(10),
                    text(format!("Reward: ${:.0}", rr.total_reward))
                        .size(11)
                        .color(Color::from_rgb(0.3, 0.8, 0.4)),
                    Space::new().width(10),
                    text(format!("R:R {:.2}:1", rr.ratio)).size(11),
                ]
                .spacing(4)
            } else {
                row![text("").size(11)]
            }
        } else {
            row![text("").size(11)]
        };

        // Error display.
        let error_col = if !panel.errors.is_empty() {
            let mut col = Column::new().spacing(2);
            for (_field, msg) in &panel.errors {
                col = col
                    .push(text(msg).size(11).color(Color::from_rgb(0.9, 0.3, 0.2)));
            }
            col
        } else {
            Column::new()
        };

        // Submit button.
        let submit_label = match panel.side {
            crate::order_panel::OrderSide::Buy => "Place Market BUY",
            crate::order_panel::OrderSide::Sell => "Place Market SELL",
        };
        let submit_row = row![
            Space::new().width(Fill),
            button(text(submit_label).size(13))
                .on_press(Message::OrderPanelSubmit)
                .padding([8, 16]),
            Space::new().width(4),
            button(text("\u{00D7}").size(14))
                .on_press(Message::OrderPanelDismiss)
                .padding([6, 10])
                .style(hover_text_button_style),
        ]
        .align_y(iced::Alignment::Center);

        // Account type indicator.
        let account_label = text("PAPER TRADING")
            .size(10)
            .color(Color::from_rgb(0.9, 0.7, 0.2));

        // Assemble full panel.
        let panel_content = column![
            side_row,
            container(Space::new().height(1)).width(Fill).style(|_t| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
                ..Default::default()
            }),
            symbol_row,
            container(Space::new().height(1)).width(Fill).style(|_t| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
                ..Default::default()
            }),
            qty_row,
            tp_section,
            container(Space::new().height(1)).width(Fill).style(|_t| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
                ..Default::default()
            }),
            sl_section,
            container(Space::new().height(1)).width(Fill).style(|_t| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
                ..Default::default()
            }),
            rr_row,
            error_col,
            container(Space::new().height(1)).width(Fill).style(|_t| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
                ..Default::default()
            }),
            account_label,
            submit_row,
        ]
        .spacing(6)
        .padding(12)
        .width(320);

        let base_panel: Element<'_, Message> = container(panel_content)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(
                    0.10, 0.10, 0.13,
                ))),
                border: iced::Border {
                    color: Color::from_rgb(0.3, 0.3, 0.35),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            })
            .into();

        // Confirmation dialog overlay
        if panel.showing_confirmation {
            let side_label = match panel.side {
                crate::order_panel::OrderSide::Buy => "BUY",
                crate::order_panel::OrderSide::Sell => "SELL",
            };
            let order_summary = format!(
                "{} {} {} at Market",
                side_label, panel.quantity, panel.symbol,
            );

            let mut details = Column::new().spacing(4);
            details = details.push(text(order_summary).size(12));
            if panel.tp_enabled && !panel.tp_value.is_empty() {
                let tp_display = if let (Some(last), Ok(val)) =
                    (panel.last_price, panel.tp_value.parse::<f64>())
                {
                    let resolved = crate::order_panel::resolve_price(
                        panel.tp_mode,
                        val,
                        last,
                        panel.side,
                        true,
                    );
                    format!("TP: {:.2}", resolved)
                } else {
                    format!("TP: {}", panel.tp_value)
                };
                details = details.push(
                    text(tp_display)
                        .size(11)
                        .color(Color::from_rgb(0.3, 0.8, 0.4)),
                );
            }
            if panel.sl_enabled && !panel.sl_value.is_empty() {
                let sl_display = if let (Some(last), Ok(val)) =
                    (panel.last_price, panel.sl_value.parse::<f64>())
                {
                    let resolved = crate::order_panel::resolve_price(
                        panel.sl_mode,
                        val,
                        last,
                        panel.side,
                        false,
                    );
                    format!("SL: {:.2}", resolved)
                } else {
                    format!("SL: {}", panel.sl_value)
                };
                details = details.push(
                    text(sl_display)
                        .size(11)
                        .color(Color::from_rgb(0.9, 0.3, 0.3)),
                );
            }

            let confirm_content = column![
                text("Confirm Market Order").size(14),
                container(Space::new().height(1))
                    .width(Fill)
                    .style(|_t| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(
                            0.3, 0.3, 0.35,
                        ))),
                        ..Default::default()
                    }),
                details,
                container(Space::new().height(1))
                    .width(Fill)
                    .style(|_t| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(
                            0.3, 0.3, 0.35,
                        ))),
                        ..Default::default()
                    }),
                row![
                    button(text("Cancel").size(12))
                        .on_press(Message::OrderPanelConfirmNo)
                        .padding([6, 16]),
                    Space::new().width(Fill),
                    button(text("Confirm & Submit").size(12))
                        .on_press(Message::OrderPanelConfirmYes)
                        .padding([6, 16]),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .padding(16)
            .width(300);

            let confirm_dialog: Element<'_, Message> = container(confirm_content)
                .style(|_theme| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgb(
                        0.12, 0.12, 0.16,
                    ))),
                    border: iced::Border {
                        color: Color::from_rgb(0.4, 0.4, 0.5),
                        width: 1.5,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                })
                .into();

            let dialog_positioned = container(confirm_dialog)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center);

            // Dark semi-transparent backdrop over the panel
            let backdrop: Element<'_, Message> = container(dialog_positioned)
                .width(320)
                .style(|_theme| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgba(
                        0.0, 0.0, 0.0, 0.6,
                    ))),
                    ..Default::default()
                })
                .into();

            return stack![base_panel, backdrop].into();
        }

        base_panel
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
    visible.sort_by(|a, b| {
        a.screen_y
            .partial_cmp(&b.screen_y)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

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
                    text(label.text.clone())
                        .size(label_font_size)
                        .color(label_color),
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
        let badge_half_height = 15.0;
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
    let price_row = iced::widget::mouse_area(price_row_inner).on_scroll(move |delta| {
        let lines = match delta {
            iced::mouse::ScrollDelta::Lines { y, .. } => y,
            iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
        };
        Message::LevelEditorPriceStep(chart_id, level_id, coarse_step * lines as f64)
    });

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
                .on_press(Message::LevelEditorThicknessChanged(chart_id, level_id, t))
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
    let lock_label = if level.locked { "Unlock" } else { "Lock" };
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

/// Compute ghost crosshair position for a sibling chart from crosshair sync.
///
/// Returns `Some((pixel_x, pixel_y))` when `sync` points to a different chart
/// with the same symbol. The Y position uses the source chart's raw cursor
/// price (same ticker = same price axis) so the horizontal arm tracks
/// smoothly instead of jumping between candle closes.
fn compute_ghost_crosshair(
    sync: &Option<(ChartId, i64, f64, String)>,
    this_chart: ChartId,
    symbol: &str,
    chart_state: &midas_chart::state::ChartState,
    data: Option<&midas_core::CandleBuffer>,
) -> Option<(f32, f32)> {
    let (src_id, ts, price, sym) = sync.as_ref()?;
    if *src_id == this_chart || sym != symbol {
        return None;
    }
    let data = data?;
    if data.is_empty() {
        return None;
    }
    let cam = &chart_state.camera;
    let gy = cam.snap_to_pixel(cam.price_to_y(*price));
    let gx = if chart_state.collapse_gaps {
        let idx = data.find_index_by_time(*ts);
        cam.time_to_x(idx as f64 + 0.5) as f32
    } else {
        cam.time_to_x(*ts as f64) as f32
    };
    Some((gx, gy))
}

/// Compute `LevelRender` data from levels and chart state for use in overlays.
fn compute_level_renders(
    levels: &[midas_chart::HorizontalLevel],
    chart: &ChartPanel,
) -> Vec<midas_chart::LevelRender> {
    let cam = &chart.chart_state.camera;
    levels
        .iter()
        .map(|lev| midas_chart::LevelRender {
            id: AnnotationId(lev.id),
            price: lev.price,
            screen_y: cam.price_to_y(lev.price),
            color: lev.color,
            line_width: lev.line_width,
            is_selected: chart.chart_state.selected_level == Some(AnnotationId(lev.id)),
            is_being_dragged: false,
            original_screen_y: None,
            label_text: midas_chart::format_price(lev.price),
            label: lev.label.clone(),
            icon: lev.icon.clone(),
            locked: lev.locked,
        })
        .collect()
}

// ── Crosshair label overlay ─────────────────────────────────────────

fn build_crosshair_label_overlay<'a>(
    labels: Option<&midas_chart::CrosshairLabels>,
    timeline_border_ratio: f32,
    viewport_width: u32,
    viewport_height: u32,
) -> Element<'a, Message> {
    let labels = match labels {
        Some(l) => l,
        None => return Space::new().width(0).height(0).into(),
    };

    let label_font_size = 11.0;
    let vw = viewport_width.max(1) as f32;
    let vh = viewport_height.max(1) as f32;
    let border_y = vh * (1.0 - timeline_border_ratio);

    let mut elements: Vec<Element<'a, Message>> = Vec::new();

    // ── Price label (right edge, centered on cursor Y) ────────────
    {
        let pl = &labels.price_label;
        let [r, g, b, a] = pl.bg_color;
        let bg = Color::from_rgba(r, g, b, a);
        let [tr, tg, tb, ta] = pl.text_color;
        let fg = Color::from_rgba(tr, tg, tb, ta);

        let badge_half_h = (label_font_size + 6.0) / 2.0;
        let top_pad = (pl.screen_y - badge_half_h)
            .max(0.0)
            .min(border_y - badge_half_h * 2.0);

        let badge = container(text(pl.text.clone()).size(label_font_size).color(fg))
            .padding([3, 6])
            .style(move |_theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        // Right-aligned: flexible spacer on the left, badge, small gap to edge.
        let positioned = container(
            row![
                Space::new().width(Fill),
                badge,
                Space::new().width(Length::Fixed(4.0))
            ]
            .width(Fill),
        )
        .padding(iced::Padding::ZERO.top(top_pad))
        .width(Fill)
        .height(Fill);

        elements.push(positioned.into());
    }

    // ── Time label (below timeline, centered on snap X) ────────────
    {
        let tl = &labels.time_label;
        let [r, g, b, a] = tl.bg_color;
        let bg = Color::from_rgba(r, g, b, a);
        let [tr, tg, tb, ta] = tl.text_color;
        let fg = Color::from_rgba(tr, tg, tb, ta);

        let badge_height = label_font_size + 6.0;
        let top_pad = (vh - badge_height - 2.0).max(0.0);

        let badge = container(text(tl.text.clone()).size(label_font_size).color(fg))
            .padding([3, 6])
            .style(move |_theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        let snap_x = tl.screen_x;
        let left_portion = ((snap_x / vw) * 1000.0) as u16;
        let right_portion = 1000_u16.saturating_sub(left_portion);

        let positioned = container(
            row![
                Space::new().width(Length::FillPortion(left_portion.max(1))),
                badge,
                Space::new().width(Length::FillPortion(right_portion.max(1))),
            ]
            .width(Fill),
        )
        .padding(iced::Padding::ZERO.top(top_pad))
        .width(Fill)
        .height(Fill);

        elements.push(positioned.into());
    }

    stack(elements).width(Fill).height(Fill).into()
}

// ── Gerchik ATR overlay ────────────────────────────────────────────

/// Build a GerchikAtrRender from the central market_cache.
/// Both chart overlay and watchlist grid read from the same source.
fn gatr_render_from_cache(
    cache: &crate::market_cache::MarketDataCache,
    symbol: &str,
) -> Option<midas_chart::GerchikAtrRender> {
    let snap = cache.get(symbol)?;
    let pct = snap.gatr_pct?;
    let price_up = snap.change_pct.map_or(true, |c| c >= 0.0);
    let color = midas_core::gatr_color(price_up);
    Some(midas_chart::GerchikAtrRender {
        pct,
        text: format!("G.ATR {:.0}%", pct),
        color,
    })
}

fn build_gerchik_atr_overlay<'a>(
    data: Option<&midas_chart::GerchikAtrRender>,
) -> Element<'a, Message> {
    let data = match data {
        Some(d) => d,
        None => return Space::new().width(0).height(0).into(),
    };

    let color = Color::from_rgba(data.color[0], data.color[1], data.color[2], data.color[3]);

    // Bold watermark-style text, offset from the right edge.
    let label = text(data.text.clone())
        .size(20)
        .color(color)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        });

    container(row![
        Space::new().width(Fill),
        label,
        Space::new().width(Length::Fixed(60.0)),
    ])
    .width(Fill)
    .padding(iced::Padding::ZERO.top(8.0))
    .into()
}

// ── Grid cell helpers ──────────────────────────────────────────────

/// Wrap content in a grid data cell with border styling.
/// Uses a fixed row height and clips overflow so text never wraps.
fn grid_data_cell<'a>(content: Element<'a, Message>, width: f32) -> Element<'a, Message> {
    container(content)
        .width(width)
        .height(28.0)
        .padding([2, 4])
        .align_y(iced::alignment::Vertical::Center)
        .clip(true)
        .style(|_| container::Style {
            border: iced::Border {
                color: midas_grid::GRID_BORDER_COLOR,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
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

/// Dark-themed pick_list style matching the toolbar background.
fn dark_pick_list_style(
    theme: &iced::Theme,
    status: pick_list::Status,
) -> pick_list::Style {
    let _ = theme;
    let bg = match status {
        pick_list::Status::Hovered => Color::from_rgba(1.0, 1.0, 1.0, 0.12),
        pick_list::Status::Opened { .. } => Color::from_rgba(1.0, 1.0, 1.0, 0.08),
        _ => Color::from_rgba(1.0, 1.0, 1.0, 0.05),
    };
    pick_list::Style {
        text_color: theme::TEXT_SECONDARY,
        placeholder_color: theme::TEXT_MUTED,
        handle_color: theme::TEXT_MUTED,
        background: iced::Background::Color(bg),
        border: iced::Border {
            color: Color::from_rgba(1.0, 1.0, 1.0, 0.15),
            width: 1.0,
            radius: 3.0.into(),
        },
    }
}

/// Button style for the active BUY side in the order panel.
fn active_buy_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.15, 0.55, 0.30))),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Button style for the active SELL side in the order panel.
fn active_sell_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.70, 0.20, 0.20))),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Button style for the inactive (unselected) side in the order panel.
fn inactive_side_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.18, 0.18, 0.22))),
        text_color: Color::from_rgb(0.6, 0.6, 0.6),
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
