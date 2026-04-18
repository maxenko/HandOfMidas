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

use midas_core::{
    ChartId, LinkColor, LinkMode, OrderBlotterId, OrderPanelId, Timeframe, WatchlistId,
};

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

        let toast_overlay = self.view_toast_overlay();

        // Drag overlay: floating label near cursor when dragging a ticker.
        if let Some(ref drag) = self.dragging_ticker {
            let label = container(text(drag.symbol.clone()).size(13).color(Color::WHITE))
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
                .padding(iced::Padding::ZERO.top(pos.y + 16.0).left(pos.x + 12.0));

            let base = column![toolbar, content, status_bar];
            return match toast_overlay {
                Some(toast) => stack![base, drag_preview, toast].into(),
                None => stack![base, drag_preview].into(),
            };
        }

        let base = column![toolbar, content, status_bar];
        match toast_overlay {
            Some(toast) => stack![base, toast].into(),
            None => base.into(),
        }
    }

    /// Build the floating toast overlay, anchored bottom-right of the
    /// main window. Returns `None` when no toast is visible.
    ///
    /// Slice 4 ships the full toast view layer from scratch: the
    /// `toast` state field exists today but was never rendered — this
    /// is what makes the GATR-snap undo affordance reach the user.
    /// Style mirrors the existing bracket-decorator badge palette for
    /// visual consistency: a darker translucent background with a
    /// subtle rounded border.
    fn view_toast_overlay(&self) -> Option<Element<'_, Message>> {
        let state = self.toast.as_ref()?;
        // Toast message: always present.
        let msg_text = text(state.message.clone()).size(13).color(Color::WHITE);

        // Action button, when present. Clicking the button fires the
        // embedded message through `Message::ToastActionClicked`,
        // which also dismisses the toast.
        let body: Element<'_, Message> = match state.action {
            Some(ref action) => {
                let action_btn = button(text(action.label.clone()).size(12).color(Color::WHITE))
                    .padding([3, 10])
                    .style(|_, status| button::Style {
                        background: Some(iced::Background::Color(match status {
                            button::Status::Hovered => Color::from_rgba(0.35, 0.50, 0.72, 1.0),
                            _ => Color::from_rgba(0.25, 0.40, 0.62, 1.0),
                        })),
                        text_color: Color::WHITE,
                        border: iced::Border {
                            color: Color::from_rgba(0.55, 0.70, 0.90, 0.9),
                            width: 1.0,
                            radius: 3.0.into(),
                        },
                        ..Default::default()
                    })
                    .on_press(Message::ToastActionClicked);
                row![
                    msg_text,
                    Space::new().width(Length::Fixed(12.0)),
                    action_btn,
                ]
                .align_y(iced::Alignment::Center)
                .into()
            }
            None => msg_text.into(),
        };

        // The toast container — styled to match the existing badge
        // palette. Clicking anywhere on it dismisses the toast via
        // `Message::DismissToast`; the inner button handles its own
        // press so its event does not reach the outer dismiss handler.
        let toast_container = container(body)
            .padding([8, 14])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(Color::from_rgba(
                    0.12, 0.14, 0.18, 0.94,
                ))),
                text_color: Some(Color::WHITE),
                border: iced::Border {
                    color: Color::from_rgba(0.30, 0.35, 0.45, 0.95),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });

        // Anchor bottom-right: use a full-size container with max
        // padding and Shrink sizing so the toast sits in the corner
        // while leaving the rest of the overlay transparent and
        // click-through.
        let positioned = container(toast_container)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .padding(16);
        Some(positioned.into())
    }

    /// Build the view for a floating (pop-out) chart window.
    fn view_floating_chart<'a>(
        &'a self,
        wid: window::Id,
        chart: &'a ChartPanel,
    ) -> Element<'a, Message> {
        // If data is loaded, render via GPU Shader widget.
        if let Some(ref data) = chart.data {
            // Compute G.ATR early so bright_ranges can be included in the snapshot.
            let gerchik_atr = gatr_render_from_cache(&self.market_cache, &chart.symbol);

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
                dragging_annotation_id: self.dragging_annotation.map(|aid| aid.0),
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
                bracket_annotations: self
                    .annotation_store
                    .get(&chart.symbol)
                    .iter()
                    .filter(|a| {
                        matches!(a.kind, midas_chart::widget::AnnotationKind::OrderBracket(_))
                    })
                    .cloned()
                    .collect(),
                gatr_bright_ranges: if chart.gatr_hover && chart.timeframe == Timeframe::D1 {
                    chart
                        .data
                        .as_ref()
                        .map_or(Vec::new(), |d| compute_daily_bright_ranges(d))
                } else {
                    Vec::new()
                },
                pinned: self
                    .tickers
                    .get(&crate::annotation_store::SymbolKey::new(&chart.symbol))
                    .map(|ts| ts.pinned())
                    .unwrap_or(false),
            };
            // Use ChartId(0) for floating windows -- they don't participate
            // in the pane_grid's chart map.
            let program = crate::chart_widget::ChartProgram {
                chart_id: ChartId::new(0),
                snapshot,
            };
            let shader = crate::chart_widget::chart_shader(program);

            // Timeline + priceline axis labels both render on GPU via
            // `midas_render::pipelines::text::TextPipeline::draw_axis`,
            // which sits BEFORE any annotation/decorator draw so axis
            // text always lives at the back of the chart.
            let camera = &chart.chart_state.camera;

            // Build level-related overlays for floating window.
            let floating_chart_id = ChartId::new(0);
            let drawing_panel = build_drawing_panel(floating_chart_id, self.level_placing);

            let mut chart_layers: Vec<Element<'_, Message>> = vec![shader.into()];

            chart_layers.push(build_gerchik_atr_overlay(
                gerchik_atr.as_ref(),
                floating_chart_id,
                chart.timeframe == Timeframe::D1,
            ));

            // Level text (price flag + icon/name) now renders on GPU
            // via `midas_render::pipelines::text::TextPipeline`, fed
            // by `scene.labels` forwarded from the decorator compute.

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
            let store_levels = self.level_store.levels_for(&chart.symbol);
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
                        iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                            .on_press(Message::DismissLinkPicker)
                            .into(),
                    );
                    let picker = self.build_link_picker(dim, move |mode| match dim {
                        LinkDimension::Symbol => Message::FloatingSetSymbolLink(wid, mode),
                        LinkDimension::Timeframe => Message::FloatingSetTimeframeLink(wid, mode),
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
            let float_s_btn = button(text("S").size(10).color(Color::WHITE).font(bold_font))
                .on_press(Message::ToggleLinkPicker(
                    PickerTarget::Floating(wid),
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

            // Timeframe link button for floating chart.
            let tf_link = chart.timeframe_link;
            let tf_color = link_mode_indicator_rgba(tf_link);
            let float_t_btn = button(text("T").size(10).color(Color::WHITE).font(bold_font))
                .on_press(Message::ToggleLinkPicker(
                    PickerTarget::Floating(wid),
                    LinkDimension::Timeframe,
                ))
                .padding([2, 5])
                .style(move |_theme, _status| button::Style {
                    background: Some(
                        Color::from_rgba(tf_color[0], tf_color[1], tf_color[2], tf_color[3]).into(),
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

        let order_btn = button(text("Order").size(12))
            .on_press(Message::AddOrderPanel)
            .padding([4, 10])
            .style(hover_text_button_style);

        let orders_btn = button(text("Orders").size(12))
            .on_press(Message::AddOrderBlotter)
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
        let broker_picker = pick_list(
            broker_names,
            Some(active_broker),
            Message::OrderBrokerSelected,
        )
        .text_size(11)
        .padding([3, 6])
        .style(dark_pick_list_style);

        let toolbar_row = row![
            layout_buttons,
            split_buttons,
            add_btn,
            wl_btn,
            order_btn,
            orders_btn,
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
                    PanelContent::Order(order_id) => {
                        let tb = self.view_order_title_bar(order_id, pane);
                        let bd = self.view_order_body(order_id);
                        (tb, bd)
                    }
                    PanelContent::OrderBlotter(blotter_id) => {
                        let tb = self.view_order_blotter_title_bar(blotter_id, pane);
                        let bd = self.view_order_blotter_body(blotter_id);
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
        let s_btn = button(text("S").size(10).color(Color::WHITE).font(bold_font))
            .on_press(Message::ToggleLinkPicker(
                PickerTarget::Docked(chart_id),
                LinkDimension::Symbol,
            ))
            .padding([2, 5])
            .style(move |_theme, _status| button::Style {
                background: Some(
                    Color::from_rgba(sym_color[0], sym_color[1], sym_color[2], sym_color[3]).into(),
                ),
                text_color: Color::WHITE,
                border: iced::Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });

        // Timeframe link button.
        let tf_link = chart
            .map(|c| c.timeframe_link)
            .unwrap_or(LinkMode::Unlinked);
        let tf_color = link_mode_indicator_rgba(tf_link);
        let t_btn = button(text("T").size(10).color(Color::WHITE).font(bold_font))
            .on_press(Message::ToggleLinkPicker(
                PickerTarget::Docked(chart_id),
                LinkDimension::Timeframe,
            ))
            .padding([2, 5])
            .style(move |_theme, _status| button::Style {
                background: Some(
                    Color::from_rgba(tf_color[0], tf_color[1], tf_color[2], tf_color[3]).into(),
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

        if let Some(ref data) = chart.data {
            // Compute G.ATR early so bright_ranges can be included in the snapshot.
            let gerchik_atr = gatr_render_from_cache(&self.market_cache, &chart.symbol);

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
                dragging_annotation_id: self.dragging_annotation.map(|aid| aid.0),
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
                bracket_annotations: self
                    .annotation_store
                    .get(&chart.symbol)
                    .iter()
                    .filter(|a| {
                        matches!(a.kind, midas_chart::widget::AnnotationKind::OrderBracket(_))
                    })
                    .cloned()
                    .collect(),
                gatr_bright_ranges: if chart.gatr_hover && chart.timeframe == Timeframe::D1 {
                    chart
                        .data
                        .as_ref()
                        .map_or(Vec::new(), |d| compute_daily_bright_ranges(d))
                } else {
                    Vec::new()
                },
                pinned: self
                    .tickers
                    .get(&crate::annotation_store::SymbolKey::new(&chart.symbol))
                    .map(|ts| ts.pinned())
                    .unwrap_or(false),
            };
            let program = crate::chart_widget::ChartProgram { chart_id, snapshot };
            let shader = crate::chart_widget::chart_shader(program);

            // Timeline + priceline axis labels render on GPU via the
            // text pipeline's axis layer.
            let camera = &chart.chart_state.camera;

            // Build level-related overlays.
            let drawing_panel = build_drawing_panel(chart_id, self.level_placing);

            let mut chart_layers: Vec<Element<'_, Message>> = vec![shader.into()];

            chart_layers.push(build_gerchik_atr_overlay(
                gerchik_atr.as_ref(),
                chart_id,
                chart.timeframe == Timeframe::D1,
            ));

            // Level text (price flag + icon/name) now renders on GPU
            // via `midas_render::pipelines::text::TextPipeline`, fed
            // by `scene.labels` forwarded from the decorator compute.

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
            let store_levels = self.level_store.levels_for(&chart.symbol);
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
                        iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                            .on_press(Message::DismissLinkPicker)
                            .into(),
                    );
                    let picker = self.build_link_picker(dim, move |mode| match dim {
                        LinkDimension::Symbol => Message::SetSymbolLink(chart_id, mode),
                        LinkDimension::Timeframe => Message::SetTimeframeLink(chart_id, mode),
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
        _dimension: LinkDimension,
        msg_builder: impl Fn(LinkMode) -> Message,
    ) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::with_capacity(10);

        // 8 color options.
        for color in LinkColor::ALL {
            let mode = LinkMode::Color(color);
            let rgba = link_color_rgba(color);
            let label = color.display_name();
            let msg = msg_builder(mode);

            let color_swatch =
                container(Space::new().width(12).height(12)).style(move |_| container::Style {
                    background: Some(Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).into()),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });

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
        let listen_swatch =
            container(Space::new().width(12).height(12)).style(move |_| container::Style {
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
            });
        items.push(
            button(
                row![listen_swatch, text("Listen *").size(11)]
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
        let gray_swatch =
            container(Space::new().width(12).height(12)).style(move |_| container::Style {
                background: Some(
                    Color::from_rgba(gray_rgba[0], gray_rgba[1], gray_rgba[2], gray_rgba[3]).into(),
                ),
                border: iced::Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });
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
        let wl_s_btn: Element<'_, Message> =
            button(text("S").size(10).color(Color::WHITE).font(bold_font))
                .on_press(Message::ToggleLinkPicker(
                    PickerTarget::Watchlist(wl_id),
                    LinkDimension::Symbol,
                ))
                .padding([2, 5])
                .style(move |_theme, _status| button::Style {
                    background: Some(
                        Color::from_rgba(wl_color[0], wl_color[1], wl_color[2], wl_color[3]).into(),
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
        .controls(Element::from(
            row![wl_s_btn, Space::new().width(4), close_btn]
                .spacing(2)
                .align_y(iced::Alignment::Center),
        ))
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
                        let price_up = snap.change_pct.is_none_or(|c| c >= 0.0);
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
        let selected_idx = wl
            .selected_symbol
            .as_ref()
            .and_then(|sym| grid_rows.iter().position(|r| r.symbol == *sym));

        // Build a temporary GridState copy with the correct selection index.
        let mut view_state = wl.grid_state.clone();
        if let Some(idx) = selected_idx {
            view_state.selection.select(idx);
        } else {
            view_state.selection.clear();
        }

        // Build grid header + body via `midas-grid` helpers.
        // (The Grid builder can't be used here because columns/rows are local
        // variables whose borrows can't escape the function. The Grid API works
        // when data lives on &self — see Phase 2.)
        use crate::watchlist::{COL_CHANGE, COL_DELETE, COL_FAV, COL_GATR, COL_PRICE, COL_TICKER};
        use midas_grid::{
            grid_body_cell, grid_body_row, grid_header_cell, HeaderStyle, ResizeHandle,
        };

        // Column definitions: (id, header_label, sortable).
        //
        // `COL_DRAG` has no matching body cell and was shifting the
        // whole header row one column left of the data. Drop it from
        // the header — the `col_widths` map still stores a width for
        // it for legacy configs, but nothing renders it.
        let col_defs: [(midas_grid::ColumnId, &str, bool); 6] = [
            (COL_FAV, "\u{2605}", false),
            (COL_TICKER, "Ticker", true),
            (COL_PRICE, "Price", true),
            (COL_CHANGE, "Chg%", true),
            (COL_GATR, "G.ATR", true),
            (COL_DELETE, "", false),
        ];

        // Match the order-blotter header: default padding + 0.5 border,
        // 11-point muted label text. Keeps the two grids visually
        // identical — one source of truth for panel chrome.
        let header_style = HeaderStyle {
            label_color: Some(theme::TEXT_SECONDARY),
            ..HeaderStyle::default()
        };
        // Favourite column is a single glyph and wants to share pixel
        // coordinates with the body star. Match the body's size and
        // zero horizontal padding so both are centred on the same axis.
        let fav_header_style = HeaderStyle {
            padding: [6, 0],
            label_size: FAV_STAR_SIZE as u16,
            align_x: Some(iced::alignment::Horizontal::Center),
            ..header_style.clone()
        };

        // Header row.
        let mut header_cells: Vec<Element<'_, Message>> = Vec::with_capacity(col_defs.len());
        for (i, &(col_id, label, sortable)) in col_defs.iter().enumerate() {
            let width = view_state.column_width(col_id);
            let sort_indicator = view_state
                .sort
                .filter(|s| s.column_id == col_id)
                .map(|s| s.direction.indicator())
                .unwrap_or("");
            let sort_msg = sortable.then(|| {
                Message::WatchlistGrid(wl_id, midas_grid::GridMessage::SortToggled(col_id))
            });
            // `col_idx` is passed in the `WATCHLIST_COLUMN_ORDER` space
            // (which still has COL_DRAG at index 0), so we offset by +1
            // to account for the DRAG column we no longer render.
            let resize = (i < col_defs.len() - 1).then(|| ResizeHandle {
                on_press: Message::WatchlistColumnResizeStart(wl_id, i + 1, 0.0),
                height: 26.0,
            });
            let style = if col_id == COL_FAV {
                &fav_header_style
            } else {
                &header_style
            };
            header_cells.push(grid_header_cell(
                label,
                width,
                sort_indicator,
                sort_msg,
                resize,
                style,
            ));
        }
        let header = Row::with_children(header_cells);

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

                // Build cells matching column order.
                let sym = row_data.symbol.clone();
                let sym_del = row_data.symbol.clone();
                let sym_drag = row_data.symbol.clone();

                let fav_btn = favorite_star_button(
                    row_data.favorite,
                    Message::WatchlistToggleFavorite(wl_id, sym),
                );

                let del_btn = button(text("\u{00D7}").size(12))
                    .on_press(Message::WatchlistRemoveTicker(wl_id, sym_del))
                    .padding([2, 4])
                    .style(hover_text_button_style);

                // Ticker cell is a drag handle — clicking it starts a drag.
                // Inner mouse_area captures the press so the outer
                // `grid_body_row` click only fires on a non-ticker cell.
                let ticker_cell = iced::widget::mouse_area(
                    text(row_data.symbol.clone())
                        .size(12)
                        .wrapping(iced::widget::text::Wrapping::None)
                        .color(theme::TEXT_PRIMARY),
                )
                .on_press(Message::WatchlistTickerPressed(wl_id, sym_drag));

                let w = |col_id| view_state.column_width(col_id);

                // Body cells match the order-blotter layout: text size
                // 12, `[4, 8]` padding, and clip(true) so values never
                // bleed into the next column.
                use iced::widget::text::Wrapping;
                let text_cell = |s: String, width: f32, color: Color| -> Element<'_, Message> {
                    grid_body_cell(
                        text(s)
                            .size(12)
                            .wrapping(Wrapping::None)
                            .color(color)
                            .into(),
                        width,
                    )
                };

                // Favourite-star cell centres both axes so the header
                // star (also centred) lines up with body stars. Width
                // is clipped for symmetry with the other cells.
                let fav_cell: Element<'_, Message> = container(fav_btn)
                    .width(w(COL_FAV))
                    .height(Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                    .clip(true)
                    .into();

                let cells: Vec<Element<'_, Message>> = vec![
                    fav_cell,
                    grid_body_cell(ticker_cell.into(), w(COL_TICKER)),
                    text_cell(
                        row_data.price_text.clone(),
                        w(COL_PRICE),
                        theme::TEXT_PRIMARY,
                    ),
                    text_cell(
                        row_data.change_text.clone(),
                        w(COL_CHANGE),
                        row_data.change_color,
                    ),
                    text_cell(row_data.gatr_text.clone(), w(COL_GATR), row_data.gatr_color),
                    grid_body_cell(del_btn.into(), w(COL_DELETE)),
                ];

                // Emit WatchlistTickerSelected directly with the symbol.
                // This avoids the sorted-index mismatch: the view knows the
                // correct symbol at each visual row position.
                let click_msg = Message::WatchlistTickerSelected(wl_id, row_data.symbol.clone());
                // Alternating row tint, matching the blotter.
                let ticker_row =
                    grid_body_row(cells, is_selected, row_idx % 2 == 0, Some(click_msg));

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

        let main_content: Element<'_, Message> =
            column![header, scrollable(body_rows).height(Fill), add_row,]
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
                let backdrop = iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
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

// ── Dockable order panel ───────────────────────────────────────────

impl MidasApp {
    /// Build the title bar for a dockable order panel pane.
    fn view_order_title_bar(
        &self,
        order_id: OrderPanelId,
        pane: pane_grid::Pane,
    ) -> pane_grid::TitleBar<'_, Message> {
        let title_text = self
            .order_panels
            .get(&order_id)
            .map(|p| {
                if p.state.symbol.is_empty() {
                    "Order".to_string()
                } else {
                    format!("Order: {}", p.state.symbol)
                }
            })
            .unwrap_or_else(|| "Order".to_string());

        // Symbol link [S] button.
        let op_link = self
            .order_panels
            .get(&order_id)
            .map(|p| p.symbol_link)
            .unwrap_or(LinkMode::Unlinked);
        let op_color = link_mode_indicator_rgba(op_link);
        let bold_font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let op_s_btn: Element<'_, Message> =
            button(text("S").size(10).color(Color::WHITE).font(bold_font))
                .on_press(Message::ToggleLinkPicker(
                    PickerTarget::Order(order_id),
                    LinkDimension::Symbol,
                ))
                .padding([2, 5])
                .style(move |_theme, _status| button::Style {
                    background: Some(
                        Color::from_rgba(op_color[0], op_color[1], op_color[2], op_color[3]).into(),
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
            row![text(title_text).size(14), Space::new().width(Fill),]
                .align_y(iced::Alignment::Center),
        )
        .controls(Element::from(
            row![op_s_btn, Space::new().width(4), close_btn]
                .spacing(2)
                .align_y(iced::Alignment::Center),
        ))
        .padding([2, 4])
        .always_show_controls()
        .style(|_theme| container::Style::default())
    }

    /// Build the body of a dockable order panel pane.
    fn view_order_body(&self, order_id: OrderPanelId) -> Element<'_, Message> {
        use crate::order_panel::OrderPanelAction;

        let panel = match self.order_panels.get(&order_id) {
            Some(p) => p,
            None => {
                return container(text("Order panel not found").size(14))
                    .center_x(Fill)
                    .center_y(Fill)
                    .into();
            }
        };
        let state = &panel.state;

        // Fetch last_price from the market cache (authoritative source).
        let last_price = self
            .market_cache
            .get(&state.symbol)
            .and_then(|snap| snap.last_price);

        let oid = order_id;

        // Entry type tabs: [Market] [Limit] [Stop] [Stop Limit].
        let entry_type = state.entry_type;
        use midas_chart::widget::order_bracket::EntryType;
        let type_btn = |label: &'static str, et: EntryType| -> Element<'_, Message> {
            let style: fn(&iced::Theme, button::Status) -> button::Style = if entry_type == et {
                active_neutral_button_style
            } else {
                inactive_side_button_style
            };
            button(text(label).size(11))
                .on_press(Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::SetEntryType(et),
                ))
                .padding([4, 8])
                .style(style)
                .into()
        };
        let entry_type_row = row![
            type_btn("Market", EntryType::Market),
            type_btn("Limit", EntryType::Limit),
            type_btn("Stop", EntryType::Stop),
            type_btn("Stop Limit", EntryType::StopLimit),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        // Side / bracket toggle buttons: [BUY] [X] [SELL].
        //
        // BUY/SELL activate bracket mode (chart bracket visible).
        // [X] deactivates bracket mode (chart bracket hidden/cached).
        let bracket_active = state.bracket_active;

        let buy_style: fn(&iced::Theme, button::Status) -> button::Style =
            if bracket_active == Some(crate::order_panel::OrderSide::Buy) {
                active_buy_button_style
            } else {
                inactive_side_button_style
            };
        let sell_style: fn(&iced::Theme, button::Status) -> button::Style =
            if bracket_active == Some(crate::order_panel::OrderSide::Sell) {
                active_sell_button_style
            } else {
                inactive_side_button_style
            };
        let x_style: fn(&iced::Theme, button::Status) -> button::Style = if bracket_active.is_none()
        {
            active_neutral_button_style
        } else {
            inactive_side_button_style
        };

        let side_row = row![
            button(text("BUY").size(14))
                .on_press(Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::SetBracketMode(Some(crate::order_panel::OrderSide::Buy),),
                ))
                .padding([8, 20])
                .style(buy_style),
            button(text("X").size(14))
                .on_press(Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::SetBracketMode(None),
                ))
                .padding([8, 8])
                .style(x_style),
            button(text("SELL").size(14))
                .on_press(Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::SetBracketMode(Some(crate::order_panel::OrderSide::Sell),),
                ))
                .padding([8, 20])
                .style(sell_style),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center);

        // Price step for mouse wheel adjustment.
        let (coarse_step, _fine_step) = midas_chart::price_step_for(last_price.unwrap_or(100.0));

        // Entry price inputs (shown for non-Market types).
        // Each row is wrapped in mouse_area for scroll-wheel adjustment.
        let entry_price_section = {
            use midas_chart::widget::order_bracket::EntryType;

            let limit_scroll = move |delta: iced::mouse::ScrollDelta| {
                let lines = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => y,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                };
                Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::StepPrice {
                        field: crate::order_panel::PriceField::LimitPrice,
                        delta: coarse_step * lines as f64,
                    },
                )
            };
            let stop_scroll = move |delta: iced::mouse::ScrollDelta| {
                let lines = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => y,
                    iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                };
                Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::StepPrice {
                        field: crate::order_panel::PriceField::StopPrice,
                        delta: coarse_step * lines as f64,
                    },
                )
            };

            let mut col = Column::new().spacing(4);
            match entry_type {
                EntryType::Market => {} // No price input needed
                EntryType::Limit => {
                    let lp_row = row![
                        text("Limit:").size(11).width(50),
                        text_input("0.00", &state.limit_price)
                            .on_input(move |val| Message::OrderPanelMsg(
                                oid,
                                OrderPanelAction::SetLimitPrice(val),
                            ))
                            .size(12)
                            .width(100),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center);
                    col = col.push(iced::widget::mouse_area(lp_row).on_scroll(limit_scroll));
                }
                EntryType::Stop => {
                    let sp_row = row![
                        text("Stop:").size(11).width(50),
                        text_input("0.00", &state.stop_price)
                            .on_input(move |val| Message::OrderPanelMsg(
                                oid,
                                OrderPanelAction::SetStopPrice(val),
                            ))
                            .size(12)
                            .width(100),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center);
                    col = col.push(iced::widget::mouse_area(sp_row).on_scroll(stop_scroll));
                }
                EntryType::StopLimit => {
                    let sp_row = row![
                        text("Stop:").size(11).width(50),
                        text_input("0.00", &state.stop_price)
                            .on_input(move |val| Message::OrderPanelMsg(
                                oid,
                                OrderPanelAction::SetStopPrice(val),
                            ))
                            .size(12)
                            .width(100),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center);
                    let lp_row = row![
                        text("Limit:").size(11).width(50),
                        text_input("0.00", &state.limit_price)
                            .on_input(move |val| Message::OrderPanelMsg(
                                oid,
                                OrderPanelAction::SetLimitPrice(val),
                            ))
                            .size(12)
                            .width(100),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center);
                    col = col
                        .push(iced::widget::mouse_area(sp_row).on_scroll(stop_scroll))
                        .push(iced::widget::mouse_area(lp_row).on_scroll(limit_scroll));
                }
            }
            col
        };

        // Show "Waiting for market data..." when bracket is active
        // but no market price is available yet.
        let bracket_waiting: Option<Element<'_, Message>> =
            if bracket_active.is_some() && last_price.is_none() {
                Some(
                    text("Waiting for market data...")
                        .size(11)
                        .color(Color::from_rgb(0.45, 0.45, 0.45))
                        .into(),
                )
            } else {
                None
            };

        // Symbol and price display.
        let price_text = last_price
            .map(|p| format!("Last: {p:.2}"))
            .unwrap_or_else(|| "Last: --".to_string());
        let symbol_row = row![
            text(format!("Symbol: {}", state.symbol)).size(12),
            Space::new().width(Fill),
            text(price_text).size(12),
        ];

        // Quantity input.
        let qty_row = row![
            text("Qty:").size(12).width(40),
            text_input("100", &state.quantity)
                .on_input(move |val| Message::OrderPanelMsg(
                    oid,
                    OrderPanelAction::SetQuantity(val),
                ))
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
            let tp_check = row![iced::widget::checkbox(state.tp_enabled,)
                .label("Take Profit")
                .on_toggle(move |val| Message::OrderPanelMsg(oid, OrderPanelAction::ToggleTp(val),))
                .size(14),];
            col = col.push(tp_check);
            if state.tp_enabled {
                let tp_input_row = row![
                    text("Price:").size(11).width(40),
                    text_input("0.00", &state.tp_value)
                        .on_input(move |val| Message::OrderPanelMsg(
                            oid,
                            OrderPanelAction::SetTpValue(val),
                        ))
                        .size(12)
                        .width(100),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);
                let tp_input = iced::widget::mouse_area(tp_input_row).on_scroll(move |delta| {
                    let lines = match delta {
                        iced::mouse::ScrollDelta::Lines { y, .. } => y,
                        iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                    };
                    Message::OrderPanelMsg(
                        oid,
                        OrderPanelAction::StepPrice {
                            field: crate::order_panel::PriceField::Tp,
                            delta: coarse_step * lines as f64,
                        },
                    )
                });
                col = col.push(tp_input);
            }
            col
        };

        // Stop Loss section.
        let sl_section = {
            let mut col = Column::new().spacing(4);
            let sl_check = row![iced::widget::checkbox(state.sl_enabled,)
                .label("Stop Loss")
                .on_toggle(move |val| Message::OrderPanelMsg(oid, OrderPanelAction::ToggleSl(val),))
                .size(14),];
            col = col.push(sl_check);
            if state.sl_enabled {
                let sl_input_row = row![
                    text("Price:").size(11).width(40),
                    text_input("0.00", &state.sl_value)
                        .on_input(move |val| Message::OrderPanelMsg(
                            oid,
                            OrderPanelAction::SetSlValue(val),
                        ))
                        .size(12)
                        .width(100),
                ]
                .spacing(6)
                .align_y(iced::Alignment::Center);
                let sl_input = iced::widget::mouse_area(sl_input_row).on_scroll(move |delta| {
                    let lines = match delta {
                        iced::mouse::ScrollDelta::Lines { y, .. } => y,
                        iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                    };
                    Message::OrderPanelMsg(
                        oid,
                        OrderPanelAction::StepPrice {
                            field: crate::order_panel::PriceField::Sl,
                            delta: coarse_step * lines as f64,
                        },
                    )
                });
                col = col.push(sl_input);
            }
            col
        };

        // Risk/Reward display.
        let rr_row = if let Some(last) = last_price {
            let tp_price = if state.tp_enabled {
                state.tp_value.parse::<f64>().ok().map(|val| {
                    crate::order_panel::resolve_price(state.tp_mode, val, last, state.side, true)
                })
            } else {
                None
            };
            let sl_price = if state.sl_enabled {
                state.sl_value.parse::<f64>().ok().map(|val| {
                    crate::order_panel::resolve_price(state.sl_mode, val, last, state.side, false)
                })
            } else {
                None
            };
            let qty = state.quantity.parse::<f64>().unwrap_or(0.0);
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
        let error_col = if !state.errors.is_empty() {
            let mut col = Column::new().spacing(2);
            for (_field, msg) in &state.errors {
                col = col.push(text(msg).size(11).color(Color::from_rgb(0.9, 0.3, 0.2)));
            }
            col
        } else {
            Column::new()
        };

        // Submit button (disabled when no symbol or market data not available).
        let submit_section: Element<'_, Message> = if state.symbol.is_empty() {
            text("No symbol")
                .size(12)
                .color(Color::from_rgb(0.5, 0.5, 0.5))
                .into()
        } else if last_price.is_none() {
            text("Market data loading...")
                .size(12)
                .color(Color::from_rgb(0.6, 0.6, 0.3))
                .into()
        } else {
            let type_label = match state.entry_type {
                midas_chart::widget::order_bracket::EntryType::Market => "Market",
                midas_chart::widget::order_bracket::EntryType::Limit => "Limit",
                midas_chart::widget::order_bracket::EntryType::Stop => "Stop",
                midas_chart::widget::order_bracket::EntryType::StopLimit => "Stop Limit",
            };
            let side_label = match state.side {
                crate::order_panel::OrderSide::Buy => "BUY",
                crate::order_panel::OrderSide::Sell => "SELL",
            };
            let submit_label = format!("Place {} {}", type_label, side_label);
            row![
                Space::new().width(Fill),
                button(text(submit_label).size(13))
                    .on_press(Message::OrderPanelMsg(oid, OrderPanelAction::Submit))
                    .padding([8, 16]),
            ]
            .align_y(iced::Alignment::Center)
            .into()
        };

        // Account type indicator.
        let account_label = text("PAPER TRADING")
            .size(10)
            .color(Color::from_rgb(0.9, 0.7, 0.2));

        // Separator helper.
        let sep = || {
            container(Space::new().height(1))
                .width(Fill)
                .style(|_t| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgb(0.25, 0.25, 0.30))),
                    ..Default::default()
                })
        };

        // Assemble form column.
        let mut form = Column::new().spacing(6).padding(12).width(Fill);

        form = form.push(entry_type_row);
        form = form.push(side_row);
        form = form.push(entry_price_section);
        if let Some(waiting_el) = bracket_waiting {
            form = form.push(waiting_el);
        }
        form = form
            .push(sep())
            .push(symbol_row)
            .push(sep())
            .push(qty_row)
            .push(tp_section)
            .push(sep())
            .push(sl_section)
            .push(sep())
            .push(rr_row)
            .push(error_col)
            .push(sep())
            .push(account_label)
            .push(submit_section);

        // Confirmation dialog rendered inline (not as overlay).
        if state.showing_confirmation {
            let side_label = match state.side {
                crate::order_panel::OrderSide::Buy => "BUY",
                crate::order_panel::OrderSide::Sell => "SELL",
            };
            let type_name = match state.entry_type {
                midas_chart::widget::order_bracket::EntryType::Market => "Market",
                midas_chart::widget::order_bracket::EntryType::Limit => "Limit",
                midas_chart::widget::order_bracket::EntryType::Stop => "Stop",
                midas_chart::widget::order_bracket::EntryType::StopLimit => "Stop Limit",
            };
            let order_summary = format!(
                "{} {} {} at {}",
                side_label, state.quantity, state.symbol, type_name,
            );

            let mut details = Column::new().spacing(4);
            details = details.push(text(order_summary).size(12));
            if state.tp_enabled && !state.tp_value.is_empty() {
                let tp_display =
                    if let (Some(last), Ok(val)) = (last_price, state.tp_value.parse::<f64>()) {
                        let resolved = crate::order_panel::resolve_price(
                            state.tp_mode,
                            val,
                            last,
                            state.side,
                            true,
                        );
                        format!("TP: {:.2}", resolved)
                    } else {
                        format!("TP: {}", state.tp_value)
                    };
                details = details.push(
                    text(tp_display)
                        .size(11)
                        .color(Color::from_rgb(0.3, 0.8, 0.4)),
                );
            }
            if state.sl_enabled && !state.sl_value.is_empty() {
                let sl_display =
                    if let (Some(last), Ok(val)) = (last_price, state.sl_value.parse::<f64>()) {
                        let resolved = crate::order_panel::resolve_price(
                            state.sl_mode,
                            val,
                            last,
                            state.side,
                            false,
                        );
                        format!("SL: {:.2}", resolved)
                    } else {
                        format!("SL: {}", state.sl_value)
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
                        background: Some(iced::Background::Color(Color::from_rgb(0.3, 0.3, 0.35,))),
                        ..Default::default()
                    }),
                details,
                container(Space::new().height(1))
                    .width(Fill)
                    .style(|_t| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(0.3, 0.3, 0.35,))),
                        ..Default::default()
                    }),
                row![
                    button(text("Cancel").size(12))
                        .on_press(Message::OrderPanelMsg(oid, OrderPanelAction::ConfirmNo))
                        .padding([6, 16]),
                    Space::new().width(Fill),
                    button(text("Confirm & Submit").size(12))
                        .on_press(Message::OrderPanelMsg(oid, OrderPanelAction::ConfirmYes))
                        .padding([6, 16]),
                ]
                .spacing(8),
            ]
            .spacing(8)
            .padding(16)
            .width(Fill);

            let confirm_section = container(confirm_content).style(|_theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.12, 0.12, 0.16))),
                border: iced::Border {
                    color: Color::from_rgb(0.4, 0.4, 0.5),
                    width: 1.5,
                    radius: 8.0.into(),
                },
                ..Default::default()
            });

            form = form.push(confirm_section);
        }

        let main_content: Element<'_, Message> = scrollable(form).into();

        // Link picker overlay (when open for this order panel).
        if let Some((PickerTarget::Order(picker_op_id), dim)) = self.link_picker_open {
            if picker_op_id == order_id {
                let backdrop = iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                    .on_press(Message::DismissLinkPicker);

                let picker = self.build_link_picker(dim, move |mode| {
                    Message::OrderPanelSetSymbolLink(order_id, mode)
                });

                return stack![
                    main_content,
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

        main_content
    }
}

// ── Order blotter panel ─────────────────────────────────────────────

impl MidasApp {
    /// Title bar for an Orders (blotter) pane.
    fn view_order_blotter_title_bar(
        &self,
        blotter_id: OrderBlotterId,
        pane: pane_grid::Pane,
    ) -> pane_grid::TitleBar<'_, Message> {
        let name = self
            .order_blotters
            .get(&blotter_id)
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "Orders".to_string());
        let row_count = self.order_blotter.len();
        let title_text = if row_count > 0 {
            format!("{name} ({row_count})")
        } else {
            name
        };

        // Symbol-link [S] button. Colour matches the link group so
        // users can see at a glance which colour they're bound to.
        let link = self
            .order_blotters
            .get(&blotter_id)
            .map(|p| p.symbol_link)
            .unwrap_or(LinkMode::Unlinked);
        let link_rgba = link_mode_indicator_rgba(link);
        let bold_font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let link_btn: Element<'_, Message> =
            button(text("S").size(10).color(Color::WHITE).font(bold_font))
                .on_press(Message::ToggleLinkPicker(
                    PickerTarget::OrderBlotter(blotter_id),
                    LinkDimension::Symbol,
                ))
                .padding([2, 5])
                .style(move |_theme, _status| button::Style {
                    background: Some(
                        Color::from_rgba(link_rgba[0], link_rgba[1], link_rgba[2], link_rgba[3])
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

        let gear_btn: Element<'_, Message> =
            button(text("⋮").size(12).color(theme::TEXT_SECONDARY))
                .on_press(Message::OrderBlotterOpenColumnSelector(blotter_id))
                .padding([2, 6])
                .style(hover_text_button_style)
                .into();

        let close_btn: Element<'_, Message> = button(text("X").size(10))
            .on_press(Message::PaneClose(pane))
            .padding([2, 6])
            .style(hover_text_button_style)
            .into();

        pane_grid::TitleBar::new(
            row![text(title_text).size(14), Space::new().width(Fill)]
                .align_y(iced::Alignment::Center),
        )
        .controls(Element::from(
            row![
                link_btn,
                Space::new().width(4),
                gear_btn,
                Space::new().width(2),
                close_btn
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center),
        ))
        .padding([2, 4])
        .always_show_controls()
        .style(|_theme| container::Style::default())
    }

    /// Body for an Orders (blotter) pane. Renders the shared
    /// [`crate::order_blotter::OrderBlotter`] via [`midas_grid::grid`].
    fn view_order_blotter_body(&self, blotter_id: OrderBlotterId) -> Element<'_, Message> {
        use crate::order_blotter::columns::{DisplayRow, OrderBlotterColumn};
        use midas_grid::GridColumn;

        let Some(panel) = self.order_blotters.get(&blotter_id) else {
            return container(text("Blotter not found").size(14)).into();
        };

        if self.order_blotter.is_empty() {
            return container(
                column![
                    text("No orders yet")
                        .size(14)
                        .color(Color::from_rgba(0.7, 0.7, 0.7, 1.0)),
                    text("Submit a bracket on a chart to see orders here.")
                        .size(11)
                        .color(Color::from_rgba(0.5, 0.5, 0.5, 1.0)),
                ]
                .spacing(6)
                .align_x(iced::Alignment::Center),
            )
            .center_x(Fill)
            .center_y(Fill)
            .into();
        }

        // Project every OrderRow → DisplayRow for render. Typical row
        // count is small (tens). Generation-aware caching is future
        // growth; for now rebuild each frame — negligible cost.
        let mut rows: Vec<DisplayRow> = self
            .order_blotter
            .rows()
            .map(DisplayRow::from_row)
            .collect();

        // Apply the grid's active sort. Default is OrderId descending
        // (set in `OrderBlotterPanel::default_grid_state`).
        if let Some(sort) = panel.grid_state.sort.as_ref() {
            if let Some(col) = OrderBlotterColumn::ALL
                .iter()
                .find(|c| c.id() == sort.column_id)
            {
                rows.sort_by(|a, b| {
                    let ord = col.compare(a, b);
                    match sort.direction {
                        midas_grid::SortDirection::Ascending => ord,
                        midas_grid::SortDirection::Descending => ord.reverse(),
                    }
                });
            }
        }

        // Hand-built table — `midas_grid::grid()` can't borrow a
        // Vec<DisplayRow> local to this view fn (see watchlist body
        // for the same constraint).

        use crate::order_blotter::columns::{
            symbol_badge, COL_AVG_FILL, COL_INSTRUCTION, COL_LAST_UPDATE, COL_LIMIT, COL_ORDER_ID,
            COL_QTY, COL_SIDE, COL_SL, COL_STATUS, COL_STOP, COL_SYMBOL, COL_TP, COL_TYPE,
        };
        use iced::widget::{scrollable, Column as IcedColumn, Row as IcedRow};
        use midas_grid::{
            grid_body_cell, grid_body_row, grid_header_cell, HeaderStyle, ResizeHandle,
        };

        const ALL_COL_DEFS: &[(midas_grid::ColumnId, &str, bool)] = &[
            (COL_SYMBOL, "Symbol", false),
            (COL_SIDE, "Side", true),
            (COL_TYPE, "Type", true),
            (COL_QTY, "Qty", true),
            (COL_AVG_FILL, "Avg Fill Price", true),
            (COL_LIMIT, "Limit Price", true),
            (COL_STOP, "Stop Price", true),
            (COL_TP, "Take Profit", true),
            (COL_SL, "Stop Loss", true),
            (COL_STATUS, "Status", true),
            (COL_LAST_UPDATE, "Last Update Time", true),
            (COL_INSTRUCTION, "Instruction", true),
            (COL_ORDER_ID, "Order ID", true),
        ];

        // Filter out hidden columns. Symbol stays always visible —
        // a blotter with no symbol cell is useless.
        let visible: Vec<(midas_grid::ColumnId, &'static str, bool, usize)> = ALL_COL_DEFS
            .iter()
            .enumerate()
            .filter(|(_, (col_id, _, _))| {
                *col_id == COL_SYMBOL || !panel.hidden_columns.contains(col_id)
            })
            .map(|(idx, (col_id, label, sortable))| (*col_id, *label, *sortable, idx))
            .collect();

        // ── Header ─────────────────────────────────────────────────
        // Blotter uses the default HeaderStyle: [6, 8] padding, 0.5px
        // border, 11-point muted label text.
        let header_style = HeaderStyle {
            label_color: Some(theme::TEXT_SECONDARY),
            ..HeaderStyle::default()
        };
        let mut header_cells: Vec<Element<'_, Message>> = Vec::with_capacity(visible.len());
        let last_idx = visible.len().saturating_sub(1);
        for (i, (col_id, label, sortable, all_col_idx)) in visible.iter().copied().enumerate() {
            let width = panel.grid_state.column_width(col_id);
            let indicator = panel
                .grid_state
                .sort
                .filter(|s| s.column_id == col_id)
                .map(|s| s.direction.indicator())
                .unwrap_or("");
            let sort_msg = sortable.then(|| {
                Message::OrderBlotterGrid(blotter_id, midas_grid::GridMessage::SortToggled(col_id))
            });
            // `all_col_idx` refers to the position in `ALL_COL_DEFS` /
            // `OrderBlotterColumn::ids()` so visibility changes don't
            // invalidate saved widths. Last visible column has no
            // resize handle (no column to drag against on the right).
            let resize = (i < last_idx).then(|| ResizeHandle {
                on_press: Message::OrderBlotterColumnResizeStart(blotter_id, all_col_idx),
                height: 26.0,
            });
            header_cells.push(grid_header_cell(
                label,
                width,
                indicator,
                sort_msg,
                resize,
                &header_style,
            ));
        }

        let header = IcedRow::with_children(header_cells);

        // ── Body rows ─────────────────────────────────────────────
        let mut body = IcedColumn::new();
        for (row_idx, r) in rows.iter().enumerate() {
            let is_selected = panel.selected_row == Some(r.order_uuid);
            let w = |id| panel.grid_state.column_width(id);

            let side_color = match r.side {
                midas_core::broker::OrderAction::Buy => Color::from_rgb(0.30, 0.54, 0.96),
                midas_core::broker::OrderAction::Sell => Color::from_rgb(0.88, 0.31, 0.27),
            };
            let status_color = match r.status {
                crate::order_blotter::OrderStatus::Filled => Color::from_rgb(0.27, 0.75, 0.47),
                crate::order_blotter::OrderStatus::Cancelled
                | crate::order_blotter::OrderStatus::Rejected => Color::from_rgb(0.91, 0.60, 0.26),
                _ => Color::from_rgb(0.78, 0.78, 0.78),
            };
            let primary = theme::TEXT_PRIMARY;

            let text_cell = |s: String,
                             size: u32,
                             width: f32,
                             color: Color,
                             right: bool|
             -> Element<'_, Message> {
                let body = text(s)
                    .size(size)
                    .color(color)
                    .wrapping(iced::widget::text::Wrapping::None);
                let aligned = if right {
                    container(body)
                        .align_x(iced::alignment::Horizontal::Right)
                        .width(Fill)
                } else {
                    container(body).width(Fill)
                };
                grid_body_cell(aligned.into(), width)
            };

            let mut cells: Vec<Element<'_, Message>> = Vec::with_capacity(visible.len());
            for (col_id, _, _, _) in visible.iter().copied() {
                let cell: Element<'_, Message> = match col_id {
                    id if id == COL_SYMBOL => {
                        grid_body_cell(symbol_badge(r.symbol.clone(), r.side).into(), w(COL_SYMBOL))
                    }
                    id if id == COL_SIDE => text_cell(
                        match r.side {
                            midas_core::broker::OrderAction::Buy => "Buy".to_owned(),
                            midas_core::broker::OrderAction::Sell => "Sell".to_owned(),
                        },
                        12,
                        w(COL_SIDE),
                        side_color,
                        false,
                    ),
                    id if id == COL_TYPE => {
                        text_cell(r.kind_text.clone(), 12, w(COL_TYPE), primary, false)
                    }
                    id if id == COL_QTY => {
                        text_cell(r.qty_text.clone(), 12, w(COL_QTY), primary, true)
                    }
                    id if id == COL_AVG_FILL => {
                        text_cell(r.avg_fill_text.clone(), 12, w(COL_AVG_FILL), primary, true)
                    }
                    id if id == COL_LIMIT => {
                        text_cell(r.limit_text.clone(), 12, w(COL_LIMIT), primary, true)
                    }
                    id if id == COL_STOP => {
                        text_cell(r.stop_text.clone(), 12, w(COL_STOP), primary, true)
                    }
                    id if id == COL_TP => {
                        text_cell(r.tp_text.clone(), 12, w(COL_TP), primary, true)
                    }
                    id if id == COL_SL => {
                        text_cell(r.sl_text.clone(), 12, w(COL_SL), primary, true)
                    }
                    id if id == COL_STATUS => text_cell(
                        r.status.as_str().to_owned(),
                        12,
                        w(COL_STATUS),
                        status_color,
                        false,
                    ),
                    id if id == COL_LAST_UPDATE => text_cell(
                        r.last_update_text.clone(),
                        11,
                        w(COL_LAST_UPDATE),
                        primary,
                        true,
                    ),
                    id if id == COL_INSTRUCTION => text_cell(
                        r.instruction_text.clone(),
                        12,
                        w(COL_INSTRUCTION),
                        primary,
                        false,
                    ),
                    id if id == COL_ORDER_ID => {
                        text_cell(r.order_id.clone(), 11, w(COL_ORDER_ID), primary, true)
                    }
                    _ => Space::new().into(),
                };
                cells.push(cell);
            }

            // Clicking anywhere in the row broadcasts that row's symbol
            // to the link group. No-op when `symbol_link == Unlinked`.
            let click_msg =
                Message::OrderBlotterRowSelected(blotter_id, r.order_uuid, r.symbol.clone());
            let row_widget = grid_body_row(cells, is_selected, row_idx % 2 == 0, Some(click_msg));
            body = body.push(row_widget);
        }

        let main_content: Element<'_, Message> = column![header, scrollable(body).height(Fill)]
            .width(Fill)
            .height(Fill)
            .into();

        // ── Overlays ─────────────────────────────────────────────
        // Three possible overlays, layered in priority: resize drag
        // surface, column-selector popup, link picker. Each is its
        // own stack layer.
        let needs_resize_overlay = self
            .resizing_blotter_column
            .map(|(id, _, _, _)| id == blotter_id)
            .unwrap_or(false);
        let needs_column_selector = self.blotter_column_selector_open == Some(blotter_id);
        let needs_link_picker = matches!(
            self.link_picker_open,
            Some((PickerTarget::OrderBlotter(id), _)) if id == blotter_id
        );

        if !needs_resize_overlay && !needs_column_selector && !needs_link_picker {
            return main_content;
        }

        let mut layers: Vec<Element<'_, Message>> = vec![main_content];

        if needs_resize_overlay {
            layers.push(
                iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                    .interaction(iced::mouse::Interaction::ResizingHorizontally)
                    .on_move(|point| Message::OrderBlotterColumnResizing(point.x))
                    .on_release(Message::OrderBlotterColumnResizeEnd)
                    .into(),
            );
        }

        if needs_column_selector {
            let backdrop = iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                .on_press(Message::OrderBlotterDismissColumnSelector);
            layers.push(backdrop.into());

            // Build the column-entry vector from `ALL_COL_DEFS`, marking
            // Symbol as the sole mandatory entry. Entry order follows the
            // slice order.
            let entries: Vec<midas_grid::ColumnEntry<'_>> = ALL_COL_DEFS
                .iter()
                .map(|(col_id, label, _)| midas_grid::ColumnEntry {
                    id: *col_id,
                    label,
                    mandatory: *col_id == COL_SYMBOL,
                })
                .collect();
            let popup = midas_grid::column_selector_popup(
                &entries,
                &panel.hidden_columns,
                move |col_id| Message::OrderBlotterToggleColumn(blotter_id, col_id),
                Message::OrderBlotterDismissColumnSelector,
            );
            layers.push(
                container(popup)
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top)
                    .padding([4, 6])
                    .width(Fill)
                    .height(Fill)
                    .into(),
            );
        }

        if needs_link_picker {
            let backdrop = iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                .on_press(Message::DismissLinkPicker);
            let picker = self.build_link_picker(LinkDimension::Symbol, move |mode| {
                Message::OrderBlotterSetSymbolLink(blotter_id, mode)
            });
            layers.push(backdrop.into());
            layers.push(
                container(picker)
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top)
                    .padding([4, 4])
                    .width(Fill)
                    .height(Fill)
                    .into(),
            );
        }

        stack(layers).width(Fill).height(Fill).into()
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
                (
                    Color::from_rgb(0.2, 0.8, 0.2),
                    format!("Broker: {broker_name}"),
                )
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
                text(format!(" {label}"))
                    .size(12)
                    .color(theme::TEXT_SECONDARY),
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
            .is_some_and(|p| p.is_connected());
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

// ── Button style helpers ────────────────────────────────────────────

// The iced-based `build_timeline_overlay` and `build_priceline_overlay`
// used to live here. They've been retired — axis labels now render on
// the GPU text pipeline. See `midas_render::pipelines::text` and
// `midas_chart::compute::priceline_labels_to_widget_labels` /
// `timeline_labels_to_widget_labels`.

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

// Level label overlays (price flag, icon/name) used to live here as
// iced widgets. They've moved to `midas_render::pipelines::text`
// (cryoglyph) so text renders on the same GPU pass as the SDF badge
// shapes, enabling per-element z-order.

// ── Level editor popup ─────────────────────────────────────────────

/// Build the floating level-editor popup that appears on right-click.
///
/// Contains price input with step buttons, label input, color presets,
/// thickness buttons, icon selector, lock toggle, and delete button.
fn build_level_editor<'a>(
    chart_id: ChartId,
    level: &crate::level_store::StoredLevel,
    screen_pos: (f32, f32),
    price_input: &str,
    viewport_width: u32,
    viewport_height: u32,
) -> Element<'a, Message> {
    let level_id = level.id;
    let (coarse_step, _fine_step) = midas_chart::price_step_for(level.line.price);

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
        let lc = level.line.stroke.color;
        let is_selected = (lc[0] - c[0]).abs() < 0.05
            && (lc[1] - c[1]).abs() < 0.05
            && (lc[2] - c[2]).abs() < 0.05;
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
        let is_sel = (level.line.stroke.width - t).abs() < 0.1;
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
        let icon_copy = *icon_variant;
        icon_buttons = icon_buttons.push(
            button(text(label).size(20))
                .on_press(Message::LevelEditorIconChanged(
                    chart_id, level_id, icon_copy,
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
        cam.time_to_x(idx as f64 + 0.5)
    } else {
        cam.time_to_x(*ts as f64)
    };
    Some((gx, gy))
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

    // ── Price lens (right edge, centered on cursor Y) ─────────────────
    {
        let pl = &labels.priceline_lens;
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
                ..Default::default()
            });

        // Right-aligned: flexible spacer on the left, badge, gap to edge.
        let positioned = container(row![Space::new().width(Fill), badge,].width(Fill))
            .padding(iced::Padding::ZERO.top(top_pad))
            .width(Fill)
            .height(Fill);

        elements.push(positioned.into());
    }

    // ── Timeline lens (aligned with timeline labels, just above border) ──
    {
        let tl = &labels.timeline_lens;
        let [r, g, b, a] = tl.bg_color;
        let bg = Color::from_rgba(r, g, b, a);
        let [tr, tg, tb, ta] = tl.text_color;
        let fg = Color::from_rgba(tr, tg, tb, ta);

        let badge_height = label_font_size + 6.0;
        // Place badge bottom edge at border_y - 4px gap (same as time_row).
        let top_pad = (border_y - 4.0 - badge_height).max(0.0);

        let badge = container(text(tl.text.clone()).size(label_font_size).color(fg))
            .padding([3, 6])
            .style(move |_theme: &iced::Theme| container::Style {
                background: Some(iced::Background::Color(bg)),
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
    let price_up = snap.change_pct.is_none_or(|c| c >= 0.0);
    let color = midas_core::gatr_color(price_up);
    Some(midas_chart::GerchikAtrRender {
        pct,
        text: format!("G.ATR {:.0}%", pct),
        color,
        bright_ranges: Vec::new(),
    })
}

/// Compute bright candle index ranges for G.ATR hover highlighting
/// on a daily chart. Each selected bar maps 1:1 to a candle index.
fn compute_daily_bright_ranges(data: &midas_core::CandleBuffer) -> Vec<(usize, usize)> {
    if data.len() < 2 {
        return Vec::new();
    }
    let highs: Vec<f64> = data.highs.iter().map(|&h| h as f64).collect();
    let lows: Vec<f64> = data.lows.iter().map(|&l| l as f64).collect();
    let closes: Vec<f64> = data.closes.iter().map(|&c| c as f64).collect();
    let Some(result) = midas_core::gerchik_gatr_detail(&highs, &lows, &closes) else {
        return Vec::new();
    };
    let mut ranges: Vec<(usize, usize)> =
        result.selected_bars.iter().map(|&idx| (idx, idx)).collect();
    // Always include today (last bar).
    let last = data.len() - 1;
    ranges.push((last, last));
    ranges.sort_unstable_by_key(|r| r.0);
    ranges
}

fn build_gerchik_atr_overlay<'a>(
    data: Option<&midas_chart::GerchikAtrRender>,
    chart_id: ChartId,
    is_daily: bool,
) -> Element<'a, Message> {
    let data = match data {
        Some(d) => d,
        None => return Space::new().width(0).height(0).into(),
    };

    let color = Color::from_rgba(data.color[0], data.color[1], data.color[2], data.color[3]);

    // Bold watermark-style text, offset from the right edge.
    // Wrapped in mouse_area to detect hover for candle dimming (D1 only).
    let mut area = iced::widget::mouse_area(text(data.text.clone()).size(20).color(color).font(
        iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        },
    ));
    if is_daily {
        area = area
            .on_enter(Message::GatrHoverEnter(chart_id))
            .on_exit(Message::GatrHoverLeave(chart_id));
    }
    let label = area;

    container(row![
        Space::new().width(Fill),
        label,
        Space::new().width(Length::Fixed(60.0)),
    ])
    .width(Fill)
    .padding(iced::Padding::ZERO.top(8.0))
    .into()
}

// ── Favourite-star widget ───────────────────────────────────────────

/// Font size of the favourite-star glyph. Chosen to leave room for a
/// digit overlay at level ≥ 1 without becoming the tallest thing in
/// the body row.
const FAV_STAR_SIZE: f32 = 20.0;

/// Warm gold reached at favourite level 5.
const FAV_GOLD: [f32; 3] = [1.00, 0.82, 0.20];

/// Dim silver at favourite level 1. Stays subtly darker than primary
/// text so the dim end of the gradient reads as "barely pinned".
const FAV_SILVER: [f32; 3] = [0.55, 0.55, 0.60];

/// Build the favourite-star cell contents for a watchlist row.
///
/// - `level == 0`: outline glyph in muted text colour, no overlay.
/// - `level 1..=5`: filled glyph, colour interpolated silver→gold with
///   brightness rising toward the gold end, level digit overlaid in
///   bold and centred inside the glyph.
fn favorite_star_button<'a>(level: u8, on_press: Message) -> Element<'a, Message> {
    use iced::widget::stack;

    let content: Element<'a, Message> = if level == 0 {
        text("\u{2606}")
            .size(FAV_STAR_SIZE)
            .color(theme::TEXT_MUTED)
            .into()
    } else {
        let star_color = favorite_star_color(level);
        let star = text("\u{2605}").size(FAV_STAR_SIZE).color(star_color);

        // The ★ glyph is denser in its lower half; the digit needs to
        // sit just below the geometric centre so it looks centred
        // inside the visible shape.
        let bold = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let digit = text(level.to_string())
            .size(10)
            .color(Color::from_rgb(0.08, 0.08, 0.10))
            .font(bold);
        let overlay = container(digit)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .padding([2, 0]);

        stack![star, overlay].into()
    };

    button(content)
        .on_press(on_press)
        .padding([1, 4])
        .style(fav_star_button_style)
        .into()
}

/// Silver→gold gradient for favourite levels `1..=5`.
fn favorite_star_color(level: u8) -> Color {
    let level = level.clamp(1, 5);
    let t = (level as f32 - 1.0) / 4.0; // 0.0 at 1, 1.0 at 5
    let r = FAV_SILVER[0] + (FAV_GOLD[0] - FAV_SILVER[0]) * t;
    let g = FAV_SILVER[1] + (FAV_GOLD[1] - FAV_SILVER[1]) * t;
    let b = FAV_SILVER[2] + (FAV_GOLD[2] - FAV_SILVER[2]) * t;
    Color::from_rgb(r, g, b)
}

/// Background hover/press feedback for the favourite-star button with
/// no text-colour override (the inner text widgets carry their own
/// colours for gradient + digit overlay).
fn fav_star_button_style(_theme: &iced::Theme, status: button::Status) -> button::Style {
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
        background,
        border: iced::Border {
            width: 0.0,
            ..Default::default()
        },
        ..Default::default()
    }
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
fn dark_pick_list_style(theme: &iced::Theme, status: pick_list::Status) -> pick_list::Style {
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

/// Button style for the active [X] (neutral) bracket toggle.
fn active_neutral_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.30, 0.30, 0.34))),
        text_color: Color::WHITE,
        border: iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
