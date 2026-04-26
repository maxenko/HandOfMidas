//! View functions for the main application.
//!
//! Builds the widget tree: toolbar, pane grid, title bars, chart body,
//! status bar, and floating chart windows.
//!
//! ## Slice 9a — feature-gate × config matrix (implemented)
//!
//! Plan `chart-transition` Scenario 9: when a build is compiled
//! WITHOUT `--features session_chart` but a user config selects
//! `backend: "New"`, the dispatch inside this module must:
//!
//! 1. Still parse the `ChartBackend` enum (enum parsing is
//!    feature-independent — lives in `midas-core`).
//! 2. Fall back to `Legacy` with a `tracing::warn!` on first encounter.
//! 3. Never panic, never silently drop the selection.
//!
//! Implemented by [`resolve_backend`] — called from every chart-panel
//! render site. Four-cell matrix covered by the dispatch tests.

use iced::widget::pane_grid::{self, PaneGrid};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, stack, text, text_input, Column, Row,
    Space,
};
use iced::{window, Color, Element, Fill, Length};

use midas_core::{
    AccountPanelId, ChartBackend, ChartId, LinkColor, LinkMode, OrderPanelId, Timeframe,
    WatchlistId,
};

/// Latches to `true` after the first config-vs-feature mismatch
/// fall-back log (plan Scenario 9). Prevents a chatty stream of
/// `tracing::warn!`s when a user reloads a config that selects `New`
/// under a build without `--features session_chart`.
///
/// Only relevant when `session_chart` is OFF — the `New` branch in
/// [`resolve_backend`] consults the latch. Feature-gated so the
/// variable never exists in builds that enable the feature (avoids a
/// dead-code warning).
#[cfg(not(feature = "session_chart"))]
static BACKEND_FALLBACK_WARNED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Resolve the effective rendering backend for a panel given the
/// stored [`ChartBackend`] and this build's feature flags (plan
/// Scenario 9 / R4).
///
/// - `Legacy` always resolves to `Legacy` (feature on or off).
/// - `New` with `--features session_chart` resolves to `New`.
/// - `New` without the feature falls back to `Legacy` with a
///   `tracing::warn!` the first time it fires per process.
///
/// Pure function — the fallback warning is the only side-effect.
///
/// The `#[allow]` is necessary because under `--features session_chart`
/// this function reduces to an identity (both arms map to themselves);
/// clippy's `needless_match` doesn't know the `#[cfg]` branches carry
/// the actual feature-gate logic.
#[allow(clippy::needless_match)]
pub(crate) fn resolve_backend(selected: ChartBackend) -> ChartBackend {
    match selected {
        ChartBackend::Legacy => ChartBackend::Legacy,
        ChartBackend::New => {
            #[cfg(feature = "session_chart")]
            {
                ChartBackend::New
            }
            #[cfg(not(feature = "session_chart"))]
            {
                if !BACKEND_FALLBACK_WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tracing::warn!(
                        "config selects New backend but build lacks session_chart feature; \
                         falling back to Legacy"
                    );
                }
                ChartBackend::Legacy
            }
        }
    }
}

/// Test-only reset for the once-per-process fallback warning latch.
/// Lets dispatch-matrix tests assert the warning fires for every
/// "feature off + New" cell without interference across tests.
#[cfg(all(test, not(feature = "session_chart")))]
pub(crate) fn reset_backend_fallback_warned() {
    BACKEND_FALLBACK_WARNED.store(false, std::sync::atomic::Ordering::Relaxed);
}

use crate::layout::PanelContent;
use crate::link::{link_color_rgba, link_mode_indicator_rgba, LinkDimension, PickerTarget};
use crate::theme;

use super::{LoadState, Message, MidasApp};

// ── Main entry point ────────────────────────────────────────────────

impl MidasApp {
    /// Build the widget tree for a given window.
    ///
    /// The main window shows toolbar + pane_grid + status bar.
    /// Floating chart windows show only the chart with a minimal header.
    pub fn view(&self, window_id: window::Id) -> Element<'_, Message> {
        // Session-chart window (Phase C, feature-gated). Slice F2 will
        // fold this into the regular per-window pane grid; until then
        // it stays as its own dedicated dispatch.
        #[cfg(feature = "session_chart")]
        if let Some(state) = self.floating_session_charts.get(&window_id) {
            return state.view(window_id);
        }

        // Slice C: dispatch by user-named window. The main window
        // shows toolbar + status bar; non-main windows render only the
        // header strip + pane grid (per-window broker / status are
        // explicit non-goals).
        if let Some(key) = self.iced_id_to_key.get(&window_id).cloned() {
            if let Some(ws) = self.windows.get(&key) {
                if !ws.is_main {
                    let header = self.view_window_header(&key, ws);
                    let content = self.view_content_for_window(ws);
                    return column![header, content].into();
                }
            }
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
        // Delegate to the toast controller; wrap its `ToastMsg` in
        // `Message::Toast` here so the controller stays parent-agnostic.
        // This is the SOLE wrapping site for `Message::Toast` in views.
        self.toasts.view().map(|el| el.map(Message::Toast))
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

        // The toolbar's Split buttons target the focused window's
        // focused pane. Slice D: payload-less Split is qualified with
        // the window key so a non-main window can split independently
        // when it has focus.
        let split_target = self
            .focused_window
            .clone()
            .unwrap_or_else(|| self.main_window_key.clone());
        let split_focus = self
            .windows
            .get(&split_target)
            .and_then(|ws| ws.layout.focus);
        let split_target_h = split_target.clone();
        let split_target_v = split_target;
        let split_buttons = row![
            button(text("Split H").size(11))
                .on_press_maybe(split_focus.map(|p| {
                    Message::PaneSplit(split_target_h.clone(), pane_grid::Axis::Horizontal, p)
                }))
                .padding([4, 6])
                .style(hover_text_button_style),
            button(text("Split V").size(11))
                .on_press_maybe(split_focus.map(|p| {
                    Message::PaneSplit(split_target_v.clone(), pane_grid::Axis::Vertical, p)
                }))
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

        let orders_btn = button(text("Account").size(12))
            .on_press(Message::AddAccountPanel)
            .padding([4, 10])
            .style(hover_text_button_style);

        // Session-aware charts (Phase B S8 + Phase C S10–S14).
        // Feature-gated. Three presets exercise every code path:
        //   BTC M1  — crypto / ContinuousAxis / Clock(M1).
        //   AAPL M5 — XNYS  / CompressedAxis / Clock(M5).
        //   SPY D1·RTH — XNYS / CompressedAxis / Session(Regular).
        #[cfg(feature = "session_chart")]
        let session_chart_btn: iced::Element<'_, Message> = {
            let btn_btc = button(text("BTC M1").size(11))
                .on_press(Message::OpenSessionChart(
                    crate::session_chart::SessionChartRequest::btc_m1(),
                ))
                .padding([4, 8])
                .style(hover_text_button_style);
            let btn_aapl = button(text("AAPL M5").size(11))
                .on_press(Message::OpenSessionChart(
                    crate::session_chart::SessionChartRequest::aapl_m5(),
                ))
                .padding([4, 8])
                .style(hover_text_button_style);
            let btn_spy = button(text("SPY D1·RTH").size(11))
                .on_press(Message::OpenSessionChart(
                    crate::session_chart::SessionChartRequest::spy_d1_rth(),
                ))
                .padding([4, 8])
                .style(hover_text_button_style);
            row![
                text("Session:").size(11).color(theme::TEXT_SECONDARY),
                btn_btc,
                btn_aapl,
                btn_spy
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into()
        };
        #[cfg(not(feature = "session_chart"))]
        let session_chart_btn: iced::Element<'_, Message> =
            iced::widget::Space::new().width(0).height(0).into();

        // Provider dropdowns (pushed to the right). Both lists +
        // active selections come from the toolbar VM.
        let toolbar_vm = self.toolbar_vm();
        let data_picker = pick_list(
            toolbar_vm.data_provider_names,
            Some(toolbar_vm.active_data_provider),
            Message::DataProviderSelected,
        )
        .text_size(11)
        .padding([3, 6])
        .style(dark_pick_list_style);

        let broker_picker = pick_list(
            toolbar_vm.broker_names,
            Some(toolbar_vm.active_broker),
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
            session_chart_btn,
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
        self.view_content_for_window(&self.windows[&self.main_window_key])
    }

    /// Render the pane-grid content area for an arbitrary window.
    ///
    /// Slice C: extracted so non-main windows can render their layout
    /// using the same widget tree the main window uses. Slice D plumbs
    /// the window key through every pane-grid message so handlers
    /// route to the right window's `pane_grid::State` instead of the
    /// implicit main-window default.
    pub(crate) fn view_content_for_window<'a>(
        &'a self,
        ws: &'a super::window_state::WindowState,
    ) -> Element<'a, Message> {
        let main_layout = &ws.layout;
        let focused_pane = main_layout.focus;
        let pane_count = main_layout.pane_count();
        let window_key = ws.key.clone();
        let on_focused = window_key.clone();
        let on_resized = window_key.clone();
        let on_dragged = window_key.clone();

        let pane_grid_widget =
            PaneGrid::new(&main_layout.panes, |pane, pane_state, _is_maximized| {
                let is_focused = focused_pane == Some(pane);

                let (title_bar, body) = match pane_state.content {
                    PanelContent::Chart(chart_id) => {
                        let tb = self.view_pane_title_bar(&window_key, chart_id, pane, pane_count);
                        let bd = self.view_pane_body(chart_id);
                        (tb, bd)
                    }
                    PanelContent::Watchlist(wl_id) => {
                        let tb = self.view_watchlist_title_bar(&window_key, wl_id, pane);
                        let bd = self.view_watchlist_body(wl_id);
                        (tb, bd)
                    }
                    PanelContent::Order(order_id) => {
                        let tb = self.view_order_title_bar(&window_key, order_id, pane);
                        let bd = self.view_order_body(order_id);
                        (tb, bd)
                    }
                    PanelContent::Account(account_id) => {
                        let tb = self.view_account_title_bar(&window_key, account_id, pane);
                        let bd = self.view_account_body(account_id);
                        (tb, bd)
                    }
                    PanelContent::Placeholder => {
                        self.view_placeholder_pane(&window_key, pane, pane_count)
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
            .on_click(move |pane| Message::PaneFocused(on_focused.clone(), pane))
            .on_resize(6, move |ev| Message::PaneResized(on_resized.clone(), ev))
            // Note: on_click fires PaneFocused for pane selection.
            // Drag-drop uses DragMouseUp with global hit-testing instead.
            .on_drag(move |ev| Message::PaneDragged(on_dragged.clone(), ev))
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
    /// Build the (title bar, body) for a placeholder pane — slice C's
    /// empty-window sentinel. Renders a centred "Click + Add Panel"
    /// hint inside an otherwise empty new window.
    fn view_placeholder_pane(
        &self,
        window_key: &midas_core::WindowKey,
        pane: pane_grid::Pane,
        pane_count: usize,
    ) -> (pane_grid::TitleBar<'_, Message>, Element<'_, Message>) {
        let body: Element<'_, Message> = container(text("Empty pane — use Add ▾").size(12))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();

        // Same close-button shape as real panes (subject to the
        // last-pane guard inside `WorkspaceLayout::close`), so
        // multi-pane windows can still drop an empty pane.
        let close_btn = button(text("×").size(14))
            .on_press(Message::PaneClose(window_key.clone(), pane))
            .padding([0, 6])
            .style(|_t, _s| iced::widget::button::Style {
                background: None,
                text_color: theme::TEXT_MUTED,
                ..Default::default()
            });
        let controls: Element<'_, Message> = if pane_count > 1 {
            iced::widget::row![close_btn].into()
        } else {
            iced::widget::Space::new().into()
        };

        let title = pane_grid::TitleBar::new(text(""))
            .controls(controls)
            .padding([2, 4])
            .always_show_controls()
            .style(|_theme| container::Style::default());

        (title, body)
    }

    /// Per-window header strip rendered above the pane grid in
    /// non-main windows (slice C). Shows the window name and the
    /// `[+ Window]` button that opens another named window.
    ///
    /// `Add ▾` for adding panels-into-this-window will land alongside
    /// the focused-window-aware Add* handlers; for the moment the
    /// pane-grid title-bar `+ Chart` buttons are still the discovery
    /// path, and `[+ Window]` here is the only header chrome that
    /// adds new app-level surfaces.
    pub(crate) fn view_window_header(
        &self,
        key: &midas_core::WindowKey,
        _ws: &super::window_state::WindowState,
    ) -> Element<'_, Message> {
        let name_label = text(key.as_str().to_string())
            .size(12)
            .color(theme::TEXT_PRIMARY);
        let new_window_btn = button(text("+ Window").size(11))
            .on_press(Message::CreateWindow { name: None })
            .padding([2, 8]);
        let row = iced::widget::row![
            container(name_label).padding([4, 8]).width(Fill),
            new_window_btn,
        ]
        .align_y(iced::alignment::Vertical::Center)
        .spacing(6);
        container(row)
            .width(Fill)
            .padding([2, 4])
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.10, 0.12, 0.16))),
                border: iced::Border {
                    color: Color::from_rgb(0.18, 0.20, 0.24),
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    /// Build the TitleBar for a pane.
    ///
    /// Layout: `[TICKER][1m|5m|...][G][R] [..drag area..] [⧉][×]`
    fn view_pane_title_bar(
        &self,
        window_key: &midas_core::WindowKey,
        chart_id: ChartId,
        pane: pane_grid::Pane,
        pane_count: usize,
    ) -> pane_grid::TitleBar<'_, Message> {
        // iced's TitleBar drag zone = title bar area NOT covered by content
        // bounds or controls bounds. Buttons in content still capture clicks.
        let title_content = self.view_title_bar_content(chart_id);
        let controls_row = self.view_title_bar_controls(window_key, chart_id, pane, pane_count);

        pane_grid::TitleBar::new(title_content)
            .controls(controls_row)
            .padding([2, 4])
            .always_show_controls()
            // Transparent — Content's background + focus border show through.
            .style(|_theme| container::Style::default())
    }

    /// Build the content (left) area of a pane's TitleBar.
    fn view_title_bar_content(&self, chart_id: ChartId) -> Element<'_, Message> {
        let vm = self.chart_pane_title_bar_vm(chart_id);
        let panel_tf = vm.timeframe;

        let ticker_input = text_input("SYMBOL", &vm.symbol_input)
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

        let collapse_active = vm.collapse_gaps;
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

        let vp_active = vm.show_volume_profile;
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

        let levels_active = vm.show_levels;
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

        // Chart-transition slice 9a: per-panel backend toggle chip.
        // Styled to match the existing toolbar chips (`session_chart_window.rs`
        // pattern: `button::primary` when active, `button::text` when
        // inactive). Label reflects the CURRENT backend; click flips
        // via [`Message::ToggleChartBackend`].
        let backend_btn = match vm.backend {
            midas_core::ChartBackend::New => button(text("New").size(10).color(Color::WHITE))
                .on_press(Message::ToggleChartBackend(chart_id))
                .padding([1, 4])
                .style(button::primary),
            midas_core::ChartBackend::Legacy => button(text("Legacy").size(10))
                .on_press(Message::ToggleChartBackend(chart_id))
                .padding([1, 4])
                .style(button::text),
        };

        row![
            ticker_input,
            tf_row,
            collapse_btn,
            vp_btn,
            levels_btn,
            reset_btn,
            backend_btn,
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
        window_key: &midas_core::WindowKey,
        chart_id: ChartId,
        pane: pane_grid::Pane,
        pane_count: usize,
    ) -> Element<'_, Message> {
        let vm = self.chart_pane_title_bar_vm(chart_id);

        // Symbol link button.
        let bold_font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let sym_link = vm.symbol_link;
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
        let tf_link = vm.timeframe_link;
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

        // Slice E: pop-out icon now opens the chart in its own
        // user-named window (replaces the legacy `floating_charts`
        // path). The handler resolves the source window via
        // `panel_to_window` so we don't need to thread the source
        // pane handle through the message — the chart id is enough.
        let _ = pane; // Pane handle no longer needed for this message.
        let pop_out_btn = button(text("\u{29C9}").size(12))
            .on_press(Message::OpenChartInNewWindow(chart_id))
            .padding([1, 5])
            .style(button::text);

        let close_btn: Element<'_, Message> = if pane_count > 1 {
            button(text("\u{00D7}").size(12))
                .on_press(Message::PaneClose(window_key.clone(), pane))
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

        // Chart-transition slice 9a: resolve per-panel backend vs
        // feature-gate (plan Scenario 9). `resolve_backend` emits the
        // once-per-process warning when the feature is off but the
        // panel selected `New`.
        let effective_backend = resolve_backend(chart.backend);
        if effective_backend == ChartBackend::New {
            return self.view_pane_body_new_backend(chart_id);
        }

        if let Some(snapshot) = self.chart_render_snapshot(chart_id) {
            let overlays = self
                .chart_pane_overlays_vm(chart_id)
                .expect("chart_render_snapshot returned Some => overlays VM also builds");

            // Snapshot is consumed by the shader Program; `data` is
            // re-borrowed off `chart` for the crosshair-labels call
            // below.
            let data = chart
                .data
                .as_ref()
                .expect("chart_render_snapshot returned Some => chart.data is Some");

            let program = crate::chart_widget::ChartProgram { chart_id, snapshot };
            let shader = crate::chart_widget::chart_shader(program);

            let camera = &chart.chart_state.camera;
            let drawing_panel = build_drawing_panel(chart_id, overlays.level_placing);

            let mut chart_layers: Vec<Element<'_, Message>> = vec![shader.into()];

            chart_layers.push(build_gerchik_atr_overlay(
                overlays.gatr.as_ref(),
                chart_id,
                chart.timeframe == Timeframe::D1,
            ));

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

            if let Some(editor) = overlays.editing_level.as_ref() {
                chart_layers.push(build_level_editor(
                    chart_id,
                    &editor.level,
                    editor.screen_pos,
                    &editor.price_input,
                    editor.viewport_width,
                    editor.viewport_height,
                ));
            }

            if let Some(dim) = overlays.link_picker_dim {
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

    /// Render the docked pane body under the session-aware new
    /// backend (chart-transition slice 9a).
    ///
    /// The floating-window session-chart path is fully wired end-to-end
    /// via [`crate::session_chart_window::SessionChartWindow::view`];
    /// the docked in-pane wiring lands in a follow-up slice that
    /// extracts the shader + driver plumbing out of the window host.
    /// For slice 9a the docked `New` panel renders a distinct
    /// placeholder so the dispatch + toggle + config-restore paths
    /// are visually verifiable and unit-testable without dragging the
    /// full session-chart surface into the pane grid.
    ///
    /// Key guarantees satisfied in this slice:
    ///
    /// - The panel paints WITHOUT panicking (plan Scenario 9 / R14).
    /// - Live bracket state is preserved in `TickerState.live_bracket`;
    ///   the new layer will seed from it on scene rebuild.
    /// - Active bracket DRAFTs were cancelled in the
    ///   `Message::SetChartBackend` handler before we got here.
    fn view_pane_body_new_backend(&self, chart_id: ChartId) -> Element<'_, Message> {
        let chart = match self.charts.get(&chart_id) {
            Some(c) => c,
            None => return self.view_empty_placeholder(),
        };
        let header_lines = vec![
            "New chart backend (session-aware)".to_string(),
            format!(
                "symbol={} timeframe={}",
                if chart.symbol.is_empty() {
                    "—"
                } else {
                    chart.symbol.as_str()
                },
                chart.timeframe.display_name(),
            ),
            "Slice 9a placeholder — docked wiring follows in a later slice".to_string(),
        ];
        let body = Column::with_children(
            header_lines
                .into_iter()
                .map(|s| text(s).size(12).color(crate::theme::TEXT_SECONDARY).into())
                .collect::<Vec<_>>(),
        )
        .spacing(4)
        .align_x(iced::alignment::Horizontal::Center);

        container(body)
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill)
            .padding(2)
            .style(|_theme| container::Style {
                background: Some(crate::theme::CHART_EMPTY_BG.into()),
                border: iced::Border {
                    color: Color::from_rgba(0.24, 0.45, 0.78, 1.0),
                    width: 1.0,
                    radius: 0.0.into(),
                },
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
    /// Build a [`crate::thumbnail_widget::ThumbnailSnapshot`] for
    /// `symbol` from the current thumbnail stores.
    ///
    /// Used at row-build time by both the watchlist and the order
    /// blotter. Reads the per-ticker interval preference from
    /// [`crate::thumbnail_store::ThumbnailStore`], the cached closes
    /// from [`crate::thumbnail_data::ThumbnailDataStore`] (read-only
    /// via `peek` — `view()` only has `&self`), and picks a trend
    /// color from the theme.
    pub(crate) fn build_thumbnail_snapshot(
        &self,
        symbol: &str,
    ) -> crate::thumbnail_widget::ThumbnailSnapshot {
        let tf = self.thumbnail_store.get(symbol);
        let entry = self.thumbnail_data.peek(symbol, tf);
        let color = thumbnail_color(&entry.closes);
        crate::thumbnail_widget::ThumbnailSnapshot {
            widget_key: crate::thumbnail_widget::widget_key(symbol, tf),
            closes: entry.closes,
            y_min: entry.y_min,
            y_max: entry.y_max,
            color,
            generation: entry.source_version,
            label: crate::thumbnail_widget::label_for_tf(tf),
        }
    }

    /// Build the TitleBar for a watchlist pane.
    fn view_watchlist_title_bar(
        &self,
        window_key: &midas_core::WindowKey,
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
            .on_press(Message::PaneClose(window_key.clone(), pane))
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
        use crate::watchlist::{
            COL_CHANGE, COL_CHART, COL_DELETE, COL_FAV, COL_GATR, COL_PRICE, COL_TICKER,
        };
        use midas_grid::{
            grid_body_cell, grid_body_row, grid_header_cell, HeaderStyle, ResizeHandle,
        };

        let Some(vm) = self.watchlist_body_vm(wl_id) else {
            return container(text("Watchlist not found").size(14))
                .center_x(Fill)
                .center_y(Fill)
                .into();
        };

        // Column definitions: (id, header_label, sortable).
        //
        // `COL_DRAG` has no matching body cell and was shifting the
        // whole header row one column left of the data. Drop it from
        // the header — the `col_widths` map still stores a width for
        // it for legacy configs, but nothing renders it.
        let col_defs: [(midas_grid::ColumnId, &str, bool); 7] = [
            (COL_FAV, "", false),
            (COL_TICKER, "Ticker", true),
            (COL_PRICE, "Price", true),
            (COL_CHANGE, "Chg%", true),
            (COL_GATR, "G.ATR", true),
            (COL_CHART, "Chart", false),
            (COL_DELETE, "", false),
        ];

        // Match the order-blotter header: default padding + 0.5 border,
        // 11-point muted label text. Keeps the two grids visually
        // identical — one source of truth for panel chrome.
        let header_style = HeaderStyle {
            label_color: Some(theme::TEXT_SECONDARY),
            ..HeaderStyle::default()
        };

        // Header row.
        let mut header_cells: Vec<Element<'_, Message>> = Vec::with_capacity(col_defs.len());
        for (i, &(col_id, label, sortable)) in col_defs.iter().enumerate() {
            let sort_indicator = vm
                .sort_indicator
                .filter(|(id, _)| *id == col_id)
                .map(|(_, ind)| ind)
                .unwrap_or("");
            let sort_msg = sortable.then(|| {
                Message::WatchlistGrid(wl_id, midas_grid::GridMessage::SortToggled(col_id))
            });
            // `col_idx` is passed in the `WATCHLIST_COLUMN_ORDER` space
            // (which still has COL_DRAG at index 0), so we offset by +1
            // to account for the DRAG column we no longer render.
            let resize = (i < col_defs.len() - 1).then(|| ResizeHandle {
                on_press: Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Begin(
                    crate::column_resize::ColumnResizeTarget::Watchlist(wl_id),
                    i + 1,
                )),
                height: 26.0,
            });
            header_cells.push(grid_header_cell(
                label,
                vm.width(col_id),
                sort_indicator,
                sort_msg,
                resize,
                &header_style,
            ));
        }
        let header = Row::with_children(header_cells);

        // Body rows.
        let mut body_rows = Column::new();
        if vm.rows.is_empty() {
            body_rows = body_rows.push(
                container(text("Add tickers to get started").size(13))
                    .padding(20)
                    .center_x(Fill),
            );
        } else {
            for (row_idx, row_data) in vm.rows.iter().enumerate() {
                let is_selected = vm.selected_row_idx == Some(row_idx);

                // Build cells matching column order.
                let sym = row_data.symbol.clone();
                let sym_del = row_data.symbol.clone();
                let sym_drag = row_data.symbol.clone();

                let fav_widget = favorite_circle(row_data.favorite);
                let fav_scroll = move |delta: iced::mouse::ScrollDelta| {
                    let lines = match delta {
                        iced::mouse::ScrollDelta::Lines { y, .. } => y,
                        iced::mouse::ScrollDelta::Pixels { y, .. } => y / 50.0,
                    };
                    let step: i8 = match lines {
                        l if l > 0.0 => 1,
                        l if l < 0.0 => -1,
                        _ => 0,
                    };
                    Message::WatchlistAdjustFavorite(wl_id, sym.clone(), step)
                };
                // `on_enter` snapshots the favourites-first sort key so
                // back-to-back wheel ticks don't slide the row out from
                // under the cursor; `on_exit` releases the snapshot.
                let fav_btn = iced::widget::mouse_area(fav_widget)
                    .on_scroll(fav_scroll)
                    .on_enter(Message::WatchlistFavCellEnter(wl_id))
                    .on_exit(Message::WatchlistFavCellExit(wl_id));

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
                    .width(vm.width(COL_FAV))
                    .height(Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center)
                    .clip(true)
                    .into();

                let thumbnail_cell_widget = crate::thumbnail_widget::thumbnail_cell(
                    row_data.thumbnail.clone(),
                    Message::ThumbnailIntervalCycle(row_data.symbol.clone()),
                );

                let cells: Vec<Element<'_, Message>> = vec![
                    fav_cell,
                    grid_body_cell(ticker_cell.into(), vm.width(COL_TICKER)),
                    text_cell(
                        row_data.price_text.clone(),
                        vm.width(COL_PRICE),
                        theme::TEXT_PRIMARY,
                    ),
                    text_cell(
                        row_data.change_text.clone(),
                        vm.width(COL_CHANGE),
                        row_data.change_color,
                    ),
                    text_cell(
                        row_data.gatr_text.clone(),
                        vm.width(COL_GATR),
                        row_data.gatr_color,
                    ),
                    grid_body_cell(thumbnail_cell_widget, vm.width(COL_CHART)),
                    grid_body_cell(del_btn.into(), vm.width(COL_DELETE)),
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
        let add_input = text_input("Add ticker...", &vm.add_ticker_input)
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

        // Wrap in `clip_layer` so the watchlist's row content (which can
        // include shader widgets like the chart-thumbnail sparkline) is
        // recorded in a renderer layer pinned to the watchlist pane's
        // bounds. Without this, fixed-width cells whose total exceeds the
        // pane width let custom shader primitives paint into adjacent
        // panes — `container.clip(true)` only clips iced quads/text, not
        // shader-pipeline output. See `midas_ui::clip_layer`.
        let main_content: Element<'_, Message> = midas_ui::clip_layer(
            column![header, scrollable(body_rows).height(Fill), add_row,]
                .width(Fill)
                .height(Fill),
        )
        .into();

        // Wrap in stack only when overlays are needed (resize or link picker).
        if !vm.show_resize_overlay && vm.link_picker_dim.is_none() {
            return main_content;
        }

        let mut body_layers: Vec<Element<'_, Message>> = vec![main_content];

        // Global resize overlay (when actively dragging a column divider).
        if vm.show_resize_overlay {
            body_layers.push(
                iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                    .interaction(iced::mouse::Interaction::ResizingHorizontally)
                    .on_move(|point| {
                        Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Move(
                            point.x,
                        ))
                    })
                    .on_release(Message::ColumnResize(
                        crate::column_resize::ColumnResizeEvent::End,
                    ))
                    .into(),
            );
        }

        let body = stack(body_layers).width(Fill).height(Fill);

        // Link picker overlay.
        if let Some(dim) = vm.link_picker_dim {
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

        body.into()
    }
}

// ── Dockable order panel ───────────────────────────────────────────

impl MidasApp {
    /// Build the title bar for a dockable order panel pane.
    fn view_order_title_bar(
        &self,
        window_key: &midas_core::WindowKey,
        order_id: OrderPanelId,
        pane: pane_grid::Pane,
    ) -> pane_grid::TitleBar<'_, Message> {
        let vm = self.order_panel_title_bar_vm(order_id);
        let title_text = vm.title_text;
        let op_color = link_mode_indicator_rgba(vm.symbol_link);
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
            .on_press(Message::PaneClose(window_key.clone(), pane))
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

        let Some(vm) = self.order_panel_body_vm(order_id) else {
            return container(text("Order panel not found").size(14))
                .center_x(Fill)
                .center_y(Fill)
                .into();
        };
        let state = vm.state;
        let last_price = vm.last_price;

        let oid = order_id;

        // Entry type tabs: [Market] [Limit] [Stop] [Stop Limit].
        let entry_type = state.entry_type;
        use midas_annotation_types::order_bracket::EntryType;
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

        // Price step for mouse wheel adjustment, pre-computed in the
        // VM off `last_price` (or the 100.0 fallback).
        let coarse_step = vm.coarse_step;

        // Entry price inputs (shown for non-Market types).
        // Each row is wrapped in mouse_area for scroll-wheel adjustment.
        let entry_price_section = {
            use midas_annotation_types::order_bracket::EntryType;

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
                midas_annotation_types::order_bracket::EntryType::Market => "Market",
                midas_annotation_types::order_bracket::EntryType::Limit => "Limit",
                midas_annotation_types::order_bracket::EntryType::Stop => "Stop",
                midas_annotation_types::order_bracket::EntryType::StopLimit => "Stop Limit",
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
                midas_annotation_types::order_bracket::EntryType::Market => "Market",
                midas_annotation_types::order_bracket::EntryType::Limit => "Limit",
                midas_annotation_types::order_bracket::EntryType::Stop => "Stop",
                midas_annotation_types::order_bracket::EntryType::StopLimit => "Stop Limit",
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

// ── Account panel (tabbed) ─────────────────────────────────────────

impl MidasApp {
    /// Title bar for an Account pane. Visually identical to the former
    /// Orders pane's title bar; displays the panel name + current row
    /// count and the link/gear/close controls.
    fn view_account_title_bar(
        &self,
        window_key: &midas_core::WindowKey,
        account_id: AccountPanelId,
        pane: pane_grid::Pane,
    ) -> pane_grid::TitleBar<'_, Message> {
        let vm = self.account_title_bar_vm(account_id);
        let title_text = if vm.row_count > 0 {
            format!("{} ({})", vm.name, vm.row_count)
        } else {
            vm.name.clone()
        };

        // Symbol-link [S] button (Orders tab's link colour).
        let link_rgba = link_mode_indicator_rgba(vm.symbol_link);
        let bold_font = iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::default()
        };
        let link_btn: Element<'_, Message> =
            button(text("S").size(10).color(Color::WHITE).font(bold_font))
                .on_press(Message::ToggleLinkPicker(
                    PickerTarget::Account(account_id),
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
                .on_press(Message::AccountOrdersOpenColumnSelector(account_id))
                .padding([2, 6])
                .style(hover_text_button_style)
                .into();

        let close_btn: Element<'_, Message> = button(text("X").size(10))
            .on_press(Message::PaneClose(window_key.clone(), pane))
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

    /// Body for an Account pane. Renders a tab strip plus the active
    /// tab's content. Slice 1 has only the Orders tab backed; other
    /// tabs show empty-state placeholders.
    fn view_account_body(&self, account_id: AccountPanelId) -> Element<'_, Message> {
        use midas_core::config::AccountTab;
        use midas_ui::{TabItem, Tabs, UiTheme};

        // Header chrome — tab badges, banner visibility — comes from a
        // pure projection so it stays unit-testable without iced.
        // `None` means the panel id is not open; the body resolution
        // below also rechecks via `account_panels.get` for the same
        // reason.
        let Some(header) = self.account_panel_header_vm(account_id) else {
            return container(text("Account panel not found").size(14)).into();
        };
        // Re-borrow the panel for the sub-tab body dispatch; the header
        // VM owns its values by now and doesn't carry a borrow.
        let panel = self
            .account_panels
            .get(&account_id)
            .expect("account_panel_header_vm returned Some => panel exists");

        let broker_connected = matches!(
            self.broker_connection_display.as_str(),
            "Ready" | "Connected"
        );

        // Hoist the recents snapshot out of the per-tab match so the
        // returned `Element` — which borrows from this slice — outlives
        // the enclosing `col.into()`. `RecentEntry: Clone` so the
        // snapshot is cheap.
        let recents_snapshot: Vec<crate::app::RecentEntry> =
            self.recent_symbols.iter().cloned().collect();

        // The Tabs widget is a midas-ui primitive. Store a theme locally so
        // the tab colours stay consistent with the rest of the UI until
        // Slice 6 threads the full theme through MidasApp.
        let ui_theme = UiTheme::default();
        let mut history_tab_item = TabItem::new("Trade History", AccountTab::TradeHistory);
        if header.history_count > 0 {
            history_tab_item = history_tab_item.with_badge(header.history_count);
        }
        let mut positions_tab_item = TabItem::new("Positions", AccountTab::Positions);
        if header.positions_count > 0 {
            positions_tab_item = positions_tab_item.with_badge(header.positions_count);
        }
        let mut recents_tab_item = TabItem::new("Recent Instruments", AccountTab::Recents);
        if header.recents_count > 0 {
            recents_tab_item = recents_tab_item.with_badge(header.recents_count);
        }
        let tabs_el: Element<'_, Message> = Tabs::new(
            vec![
                positions_tab_item,
                TabItem::new("Orders", AccountTab::Orders).with_badge(header.working_count),
                history_tab_item,
                recents_tab_item,
            ],
            header.active_tab,
            move |tab| {
                Message::Account(
                    account_id,
                    crate::account_panel::AccountMsg::TabSelected(tab),
                )
            },
        )
        .view(&ui_theme);

        // Tab strip container — no bottom padding/border so the active
        // tab's underline sits flush with the body (grid) below it.
        let tab_strip: Element<'_, Message> = container(tabs_el)
            .padding(iced::Padding {
                top: 4.0,
                right: 8.0,
                bottom: 0.0,
                left: 8.0,
            })
            .width(Fill)
            .into();

        // Active-tab body.
        let body: Element<'_, Message> = match header.active_tab {
            AccountTab::Orders => self.view_account_orders_tab(account_id),
            AccountTab::Positions => {
                // Slice 5: live Positions grid driven by `self.positions`.
                // The per-panel cache was rebuilt by the update()
                // path (subscription batches, single-event apply, or
                // TabSelected); `view()` stays pure-`&self`.
                panel
                    .positions
                    .view(&ui_theme, account_id, broker_connected)
                    .map(move |m| Message::Account(account_id, m))
            }
            AccountTab::TradeHistory => self.view_account_history_tab(account_id),
            AccountTab::Recents => self.view_account_recents_tab(account_id),
        };
        // `recents_snapshot` is only consumed inside the recents tab
        // now (via `self.recent_symbols` directly), but we still clone
        // it above for lifetime hygiene in case future paths need it.
        let _ = recents_snapshot;

        // Disconnect banner — renders above the tab strip when the
        // broker is offline AND the user has not dismissed the banner
        // for this disconnect episode. Per the plan, the banner does
        // NOT auto-dismiss on reconnect.
        let banner: Option<Element<'_, Message>> = if header.show_disconnect_banner {
            let warning_bg = ui_theme.warning_bg;
            let warning_text = ui_theme.warning_text;
            let dismiss_msg = Message::Account(
                account_id,
                crate::account_panel::AccountMsg::DisconnectBannerDismissed,
            );
            let banner_el: Element<'_, Message> = container(
                iced::widget::row![
                    text("Disconnected — data may be stale")
                        .size(13)
                        .color(warning_text),
                    Space::new().width(Fill),
                    button(text("\u{00D7}").size(14).color(warning_text))
                        .on_press(dismiss_msg)
                        .padding([0, 6])
                        .style(move |_theme, _status| iced::widget::button::Style {
                            background: None,
                            text_color: warning_text,
                            ..Default::default()
                        }),
                ]
                .align_y(iced::Alignment::Center)
                .spacing(8),
            )
            .padding([6, 12])
            .width(Fill)
            .style(move |_theme| container::Style {
                background: Some(warning_bg.into()),
                ..Default::default()
            })
            .into();
            Some(banner_el)
        } else {
            None
        };

        let mut col = Column::new().width(Fill).height(Fill);
        if let Some(b) = banner {
            col = col.push(b);
        }
        col = col.push(tab_strip).push(body);
        let main_content: Element<'_, Message> = col.into();

        // Link-picker overlay — rendered at the body level (not inside
        // each tab's view) so the [S] symbol-link button in the title
        // bar works regardless of which tab is active. Previously the
        // picker was scoped to the Orders tab, which silently swallowed
        // clicks when any other tab was active.
        let needs_link_picker = matches!(
            self.link_picker_open,
            Some((PickerTarget::Account(id), _)) if id == account_id
        );
        if !needs_link_picker {
            return main_content;
        }

        let backdrop = iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
            .on_press(Message::DismissLinkPicker);
        let picker = self.build_link_picker(LinkDimension::Symbol, move |mode| {
            Message::AccountOrdersSetSymbolLink(account_id, mode)
        });
        stack(vec![
            main_content,
            backdrop.into(),
            container(picker)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Top)
                .padding([4, 4])
                .width(Fill)
                .height(Fill)
                .into(),
        ])
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Orders-tab body. Identical render path to the former Orders pane
    /// (14 columns, sort, selection, resize, column-selector popup,
    /// link-picker overlay, empty-state, thumbnail cells).
    fn view_account_orders_tab(&self, account_id: AccountPanelId) -> Element<'_, Message> {
        use crate::order_blotter::columns::{
            symbol_badge, COL_AVG_FILL, COL_CHART, COL_INSTRUCTION, COL_LAST_UPDATE, COL_LIMIT,
            COL_ORDER_ID, COL_QTY, COL_SIDE, COL_SL, COL_STATUS, COL_STOP, COL_SYMBOL, COL_TP,
            COL_TYPE,
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
            (COL_CHART, "Chart", false),
        ];

        let Some(vm) = self.account_orders_tab_vm(account_id, ALL_COL_DEFS) else {
            return container(text("Account panel not found").size(14)).into();
        };

        if vm.is_empty {
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

        // ── Header ─────────────────────────────────────────────────
        let header_style = HeaderStyle {
            label_color: Some(theme::TEXT_SECONDARY),
            ..HeaderStyle::default()
        };
        let mut header_cells: Vec<Element<'_, Message>> =
            Vec::with_capacity(vm.visible_columns.len());
        let last_idx = vm.visible_columns.len().saturating_sub(1);
        for (i, vc) in vm.visible_columns.iter().copied().enumerate() {
            let indicator = vm
                .sort_indicator
                .filter(|(col_id, _)| *col_id == vc.id)
                .map(|(_, ind)| ind)
                .unwrap_or("");
            let sort_msg = vc.sortable.then(|| {
                Message::Account(
                    account_id,
                    crate::account_panel::AccountMsg::Orders(midas_grid::GridMessage::SortToggled(
                        vc.id,
                    )),
                )
            });
            let resize = (i < last_idx).then(|| ResizeHandle {
                on_press: Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Begin(
                    crate::column_resize::ColumnResizeTarget::AccountOrders(account_id),
                    vc.all_col_idx,
                )),
                height: 26.0,
            });
            header_cells.push(grid_header_cell(
                vc.label,
                vm.width(vc.id),
                indicator,
                sort_msg,
                resize,
                &header_style,
            ));
        }

        let header = IcedRow::with_children(header_cells);

        // ── Body rows ─────────────────────────────────────────────
        let mut body = IcedColumn::new();
        for (row_idx, r) in vm.sorted_rows.iter().enumerate() {
            let is_selected = vm.selected_row == Some(r.order_uuid);

            let side_color = match r.side {
                midas_broker::OrderAction::Buy => Color::from_rgb(0.30, 0.54, 0.96),
                midas_broker::OrderAction::Sell => Color::from_rgb(0.88, 0.31, 0.27),
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

            let mut cells: Vec<Element<'_, Message>> = Vec::with_capacity(vm.visible_columns.len());
            for vc in vm.visible_columns.iter().copied() {
                let col_id = vc.id;
                let cell: Element<'_, Message> = match col_id {
                    id if id == COL_SYMBOL => grid_body_cell(
                        symbol_badge(r.symbol.clone(), r.side).into(),
                        vm.width(COL_SYMBOL),
                    ),
                    id if id == COL_SIDE => text_cell(
                        match r.side {
                            midas_broker::OrderAction::Buy => "Buy".to_owned(),
                            midas_broker::OrderAction::Sell => "Sell".to_owned(),
                        },
                        12,
                        vm.width(COL_SIDE),
                        side_color,
                        false,
                    ),
                    id if id == COL_TYPE => {
                        text_cell(r.kind_text.clone(), 12, vm.width(COL_TYPE), primary, false)
                    }
                    id if id == COL_QTY => {
                        text_cell(r.qty_text.clone(), 12, vm.width(COL_QTY), primary, true)
                    }
                    id if id == COL_AVG_FILL => text_cell(
                        r.avg_fill_text.clone(),
                        12,
                        vm.width(COL_AVG_FILL),
                        primary,
                        true,
                    ),
                    id if id == COL_LIMIT => {
                        text_cell(r.limit_text.clone(), 12, vm.width(COL_LIMIT), primary, true)
                    }
                    id if id == COL_STOP => {
                        text_cell(r.stop_text.clone(), 12, vm.width(COL_STOP), primary, true)
                    }
                    id if id == COL_TP => {
                        text_cell(r.tp_text.clone(), 12, vm.width(COL_TP), primary, true)
                    }
                    id if id == COL_SL => {
                        text_cell(r.sl_text.clone(), 12, vm.width(COL_SL), primary, true)
                    }
                    id if id == COL_STATUS => text_cell(
                        r.status.as_str().to_owned(),
                        12,
                        vm.width(COL_STATUS),
                        status_color,
                        false,
                    ),
                    id if id == COL_LAST_UPDATE => text_cell(
                        r.last_update_text.clone(),
                        11,
                        vm.width(COL_LAST_UPDATE),
                        primary,
                        true,
                    ),
                    id if id == COL_INSTRUCTION => text_cell(
                        r.instruction_text.clone(),
                        12,
                        vm.width(COL_INSTRUCTION),
                        primary,
                        false,
                    ),
                    id if id == COL_ORDER_ID => text_cell(
                        r.order_id.clone(),
                        11,
                        vm.width(COL_ORDER_ID),
                        primary,
                        true,
                    ),
                    id if id == COL_CHART => {
                        // Thumbnail snapshot is parallel-indexed with
                        // sorted_rows in the VM — the view does no
                        // store lookup itself.
                        let snapshot = vm.row_thumbnails[row_idx].clone();
                        let thumb = crate::thumbnail_widget::thumbnail_cell(
                            snapshot,
                            Message::ThumbnailIntervalCycle(r.symbol.clone()),
                        );
                        grid_body_cell(thumb, vm.width(COL_CHART))
                    }
                    _ => Space::new().into(),
                };
                cells.push(cell);
            }

            let click_msg =
                Message::AccountOrdersRowSelected(account_id, r.order_uuid, r.symbol.clone());
            let row_widget = grid_body_row(cells, is_selected, row_idx % 2 == 0, Some(click_msg));
            body = body.push(row_widget);
        }

        // Stable scrollable ID — unique per Account pane so switching
        // tabs preserves scroll position, and multiple Account panes
        // don't share state.
        let scroll_id: iced::widget::Id = format!("account-{}-orders", account_id.0).into();
        let main_content: Element<'_, Message> =
            column![header, scrollable(body).id(scroll_id).height(Fill)]
                .width(Fill)
                .height(Fill)
                .into();

        // ── Overlays ─────────────────────────────────────────────
        // Link-picker is rendered at the body level in `view_account_body`
        // so it works across all tabs, not just Orders. Here we only
        // handle the two Orders-specific overlays: column-resize drag
        // and column-visibility popup. Both flags are pre-projected
        // into the VM above.
        if !vm.show_resize_overlay && !vm.show_column_selector {
            return main_content;
        }

        let mut layers: Vec<Element<'_, Message>> = vec![main_content];

        if vm.show_resize_overlay {
            layers.push(
                iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                    .interaction(iced::mouse::Interaction::ResizingHorizontally)
                    .on_move(|point| {
                        Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Move(
                            point.x,
                        ))
                    })
                    .on_release(Message::ColumnResize(
                        crate::column_resize::ColumnResizeEvent::End,
                    ))
                    .into(),
            );
        }

        if vm.show_column_selector {
            let backdrop = iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                .on_press(Message::AccountOrdersDismissColumnSelector);
            layers.push(backdrop.into());

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
                &vm.hidden_columns,
                move |col_id| Message::AccountOrdersToggleColumn(account_id, col_id),
                Message::AccountOrdersDismissColumnSelector,
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

        stack(layers).width(Fill).height(Fill).into()
    }

    /// Trade-History-tab body. Mirrors the Orders-tab render path
    /// (standard `midas_grid` helpers: header with resize handles,
    /// alternating body rows) so History looks and feels like Orders
    /// and the watchlist. Read-only v1 — no sort, no selection, no
    /// link-picker, no column-selector popup.
    fn view_account_history_tab(&self, account_id: AccountPanelId) -> Element<'_, Message> {
        use crate::account_panel::history_columns::HistoryColumn;
        use iced::widget::{scrollable, Column as IcedColumn, Row as IcedRow};
        use midas_grid::{
            grid_body_cell, grid_body_row, grid_header_cell, GridColumn, HeaderStyle, ResizeHandle,
        };

        let Some(vm) = self.account_history_tab_vm(account_id) else {
            return container(text("Account panel not found").size(14)).into();
        };

        if vm.is_empty() {
            return container(
                column![
                    text("No trade history yet")
                        .size(14)
                        .color(Color::from_rgba(0.7, 0.7, 0.7, 1.0)),
                    text("Filled, cancelled, and rejected orders land here.")
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

        let columns: &'static [HistoryColumn; 6] = &HistoryColumn::ALL;
        let last_idx = columns.len().saturating_sub(1);

        // ── Header ─────────────────────────────────────────────────
        let header_style = HeaderStyle {
            label_color: Some(theme::TEXT_SECONDARY),
            ..HeaderStyle::default()
        };
        let mut header_cells: Vec<Element<'_, Message>> = Vec::with_capacity(columns.len());
        for (i, col) in columns.iter().enumerate() {
            let label: &'static str = match col {
                HistoryColumn::Timestamp => "Time",
                HistoryColumn::Symbol => "Symbol",
                HistoryColumn::Side => "Side",
                HistoryColumn::Qty => "Qty",
                HistoryColumn::FillPrice => "Fill Price",
                HistoryColumn::Status => "Status",
            };
            // No sort in v1 — pass `None` and leave the indicator empty.
            let resize = (i < last_idx).then(|| ResizeHandle {
                on_press: Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Begin(
                    crate::column_resize::ColumnResizeTarget::AccountHistory(account_id),
                    i,
                )),
                height: 26.0,
            });
            header_cells.push(grid_header_cell(
                label,
                vm.column_widths[i],
                "",
                None::<Message>,
                resize,
                &header_style,
            ));
        }
        let header = IcedRow::with_children(header_cells);

        // ── Body rows ─────────────────────────────────────────────
        let mut body = IcedColumn::new();
        for (row_idx, r) in vm.rows.iter().enumerate() {
            let mut cells: Vec<Element<'_, Message>> = Vec::with_capacity(columns.len());
            for (col_idx, col) in columns.iter().enumerate() {
                // Reuse the column's own `cell()` for rendering — it already
                // handles colour tinting for Side and Status. Map the
                // AccountMsg output (column emits into its own scope) into
                // the outer Message universe; the History column never
                // actually emits messages (read-only), so the mapper is
                // unreachable, but types need to line up.
                let inner: Element<'_, Message> = {
                    let account_inner: Element<'_, crate::account_panel::AccountMsg> =
                        col.cell(r, row_idx);
                    account_inner.map(move |m| Message::Account(account_id, m))
                };
                cells.push(grid_body_cell(inner, vm.column_widths[col_idx]));
            }
            body = body.push(grid_body_row(cells, false, row_idx % 2 == 0, None));
        }

        // Stable scrollable ID — unique per Account pane; switching tabs
        // preserves scroll position and multiple Account panes don't
        // share state.
        let scroll_id: iced::widget::Id = format!("account-{}-history", account_id.0).into();
        let main_content: Element<'_, Message> =
            column![header, scrollable(body).id(scroll_id).height(Fill)]
                .width(Fill)
                .height(Fill)
                .into();

        // ── Resize overlay ────────────────────────────────────────
        if !vm.show_resize_overlay {
            return main_content;
        }

        let mut layers: Vec<Element<'_, Message>> = vec![main_content];
        layers.push(
            iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                .interaction(iced::mouse::Interaction::ResizingHorizontally)
                .on_move(|point| {
                    Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Move(point.x))
                })
                .on_release(Message::ColumnResize(
                    crate::column_resize::ColumnResizeEvent::End,
                ))
                .into(),
        );
        stack(layers).width(Fill).height(Fill).into()
    }

    /// Recent-Instruments-tab body. Two-column grid (Ticker, Last Seen)
    /// using the same `midas_grid` helpers as Orders/History/Watchlist,
    /// minus the favourite and delete columns. Clicking a row emits
    /// `AccountMsg::RecentClicked` which re-selects the symbol on the
    /// focused chart.
    fn view_account_recents_tab(&self, account_id: AccountPanelId) -> Element<'_, Message> {
        use iced::widget::{scrollable, text::Wrapping, Column as IcedColumn, Row as IcedRow};
        use midas_grid::{
            grid_body_cell, grid_body_row, grid_header_cell, HeaderStyle, ResizeHandle,
        };

        let Some(vm) = self.account_recents_tab_vm(account_id) else {
            return container(text("Account panel not found").size(14)).into();
        };

        if vm.is_empty() {
            return container(
                column![
                    text("No recent instruments yet")
                        .size(14)
                        .color(Color::from_rgba(0.7, 0.7, 0.7, 1.0)),
                    text(
                        "Switch a chart's symbol, or add a ticker to a watchlist, \
                          to populate this list."
                    )
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

        // Match the watchlist/orders grid chrome.
        let header_style = HeaderStyle {
            label_color: Some(theme::TEXT_SECONDARY),
            ..HeaderStyle::default()
        };

        // Column labels. Width comes from the VM, which already lifted
        // it out of the panel's grid_state.
        let col_labels: [&str; 2] = ["Ticker", "Last Seen"];
        let last_idx = col_labels.len().saturating_sub(1);

        // ── Header ─────────────────────────────────────────────────
        let mut header_cells: Vec<Element<'_, Message>> = Vec::with_capacity(col_labels.len());
        for (i, label) in col_labels.into_iter().enumerate() {
            let resize = (i < last_idx).then(|| ResizeHandle {
                on_press: Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Begin(
                    crate::column_resize::ColumnResizeTarget::AccountRecents(account_id),
                    i,
                )),
                height: 26.0,
            });
            header_cells.push(grid_header_cell(
                label,
                vm.column_widths[i],
                "",
                None::<Message>,
                resize,
                &header_style,
            ));
        }
        let header = IcedRow::with_children(header_cells);

        // ── Body rows ─────────────────────────────────────────────
        let mut body = IcedColumn::new();
        for (row_idx, row) in vm.rows.iter().enumerate() {
            let ticker_cell: Element<'_, Message> = grid_body_cell(
                text(row.symbol.clone())
                    .size(12)
                    .wrapping(Wrapping::None)
                    .color(theme::TEXT_PRIMARY)
                    .into(),
                vm.column_widths[0],
            );
            let last_seen_cell: Element<'_, Message> = grid_body_cell(
                text(row.last_seen_label.clone())
                    .size(12)
                    .wrapping(Wrapping::None)
                    .color(theme::TEXT_SECONDARY)
                    .into(),
                vm.column_widths[1],
            );
            let click_msg = Message::Account(
                account_id,
                crate::account_panel::AccountMsg::RecentClicked(row.symbol.clone()),
            );
            body = body.push(grid_body_row(
                vec![ticker_cell, last_seen_cell],
                false,
                row_idx % 2 == 0,
                Some(click_msg),
            ));
        }

        let scroll_id: iced::widget::Id = format!("account-{}-recents", account_id.0).into();
        let main_content: Element<'_, Message> =
            column![header, scrollable(body).id(scroll_id).height(Fill)]
                .width(Fill)
                .height(Fill)
                .into();

        // ── Resize overlay ────────────────────────────────────────
        if !vm.show_resize_overlay {
            return main_content;
        }

        let mut layers: Vec<Element<'_, Message>> = vec![main_content];
        layers.push(
            iced::widget::mouse_area(Space::new().width(Fill).height(Fill))
                .interaction(iced::mouse::Interaction::ResizingHorizontally)
                .on_move(|point| {
                    Message::ColumnResize(crate::column_resize::ColumnResizeEvent::Move(point.x))
                })
                .on_release(Message::ColumnResize(
                    crate::column_resize::ColumnResizeEvent::End,
                ))
                .into(),
        );
        stack(layers).width(Fill).height(Fill).into()
    }
}

// ── Status bar ──────────────────────────────────────────────────────

impl MidasApp {
    /// Build the status bar at the bottom of the window.
    fn view_status_bar(&self) -> Element<'_, Message> {
        let vm = self.status_bar_vm();
        let conn_block = |block: crate::view_models::status_bar::ConnectionBlockVm| {
            row![
                text("\u{25CF}").size(10).color(block.dot_color),
                text(format!(" {}", block.label))
                    .size(12)
                    .color(theme::TEXT_SECONDARY),
            ]
            .align_y(iced::Alignment::Center)
        };

        let status_row = row![
            conn_block(vm.data_connection),
            text(" | ").size(12).color(theme::TEXT_MUTED),
            conn_block(vm.broker_connection),
            text(" | ").size(12).color(theme::TEXT_MUTED),
            text(vm.status_message)
                .size(12)
                .color(theme::TEXT_SECONDARY),
            Space::new().width(Fill),
            text(format!(
                "{} | {} pane(s){} | {}",
                vm.active_info, vm.pane_count, vm.overlay_indicator, vm.current_time,
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
    level: &crate::annotation_store::StoredLevel,
    screen_pos: (f32, f32),
    price_input: &str,
    viewport_width: u32,
    viewport_height: u32,
) -> Element<'a, Message> {
    let level_id = level.id;
    let (coarse_step, _fine_step) = midas_annotation_types::price_step_for(level.line.price);

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
    for icon_variant in midas_annotation_types::LevelIcon::all() {
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
pub(crate) fn compute_ghost_crosshair(
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
pub(crate) fn gatr_render_from_cache(
    cache: &crate::market_cache::MarketDataCache,
    symbol: &str,
) -> Option<midas_chart::GerchikAtrRender> {
    let key = crate::annotation_store::SymbolKey::new(symbol);
    let snap = cache.get(&key)?;
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
pub(crate) fn compute_daily_bright_ranges(data: &midas_core::CandleBuffer) -> Vec<(usize, usize)> {
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

// ── Favourite circle widget ─────────────────────────────────────────

/// Diameter of the favourite circle, in logical px.
const FAV_CIRCLE_SIZE: f32 = 18.0;

/// Warm gold reached at favourite level 5.
const FAV_GOLD: [f32; 3] = [1.00, 0.82, 0.20];

/// Dim silver at favourite level 1.
const FAV_SILVER: [f32; 3] = [0.55, 0.55, 0.60];

/// Build the favourite-circle cell contents for a watchlist row.
///
/// - `level == 0`: empty outline circle in muted colour, no digit.
/// - `level 1..=5`: filled circle, colour interpolated silver→gold,
///   level digit drawn bold and centred inside the circle.
///
/// The caller wraps this in a `mouse_area` with an `on_scroll` handler
/// to drive the level up/down via the wheel.
fn favorite_circle<'a>(level: u8) -> Element<'a, Message> {
    let bold = iced::Font {
        weight: iced::font::Weight::Bold,
        ..iced::Font::default()
    };

    if level == 0 {
        container(iced::widget::Space::new())
            .width(FAV_CIRCLE_SIZE)
            .height(FAV_CIRCLE_SIZE)
            .style(|_| container::Style {
                background: None,
                border: iced::Border {
                    color: theme::TEXT_MUTED,
                    width: 1.2,
                    radius: (FAV_CIRCLE_SIZE / 2.0).into(),
                },
                ..Default::default()
            })
            .into()
    } else {
        let fill = favorite_circle_color(level);
        let digit = text(level.to_string())
            .size(11)
            .color(Color::from_rgb(0.08, 0.08, 0.10))
            .font(bold);
        container(digit)
            .width(FAV_CIRCLE_SIZE)
            .height(FAV_CIRCLE_SIZE)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(fill)),
                border: iced::Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: (FAV_CIRCLE_SIZE / 2.0).into(),
                },
                ..Default::default()
            })
            .into()
    }
}

/// Silver→gold gradient for favourite levels `1..=5`.
fn favorite_circle_color(level: u8) -> Color {
    let level = level.clamp(1, 5);
    let t = (level as f32 - 1.0) / 4.0; // 0.0 at 1, 1.0 at 5
    let r = FAV_SILVER[0] + (FAV_GOLD[0] - FAV_SILVER[0]) * t;
    let g = FAV_SILVER[1] + (FAV_GOLD[1] - FAV_SILVER[1]) * t;
    let b = FAV_SILVER[2] + (FAV_GOLD[2] - FAV_SILVER[2]) * t;
    Color::from_rgb(r, g, b)
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

/// Pick the thumbnail fill color for the given trailing-closes slice.
///
/// `first > last` → down-trend (red), `first < last` → up-trend (green),
/// equal / empty → flat (muted grey). The thresholds use strict
/// inequalities so truly-equal closes land on the flat color rather
/// than the slightly warmer up color.
///
/// Slice 8d of chart-transition: the lookup now routes through
/// [`theme::ThumbnailPalette`] so this surface and the main chart
/// read from the SAME palette instance. Prior to slice 8d each side
/// read its own constants and could drift under a theme swap (plan
/// R9).
fn thumbnail_color(closes: &[f32]) -> [f32; 4] {
    let palette = theme::ThumbnailPalette::dark_default();
    palette.color_for_closes(closes.first().copied(), closes.last().copied())
}

// ── Chart-transition slice 9a: backend-dispatch tests ────────────────
//
// Covers plan Scenario 9 — the four-cell feature-gate × config matrix
// (feature on/off × config Legacy/New). The tests split along `cfg`
// so each test only compiles under the feature-flag value it
// exercises: without the feature, `New` must fall back to `Legacy`
// and emit the one-shot warning; with the feature, `New` stays `New`.
#[cfg(test)]
mod backend_dispatch_tests {
    use super::{resolve_backend, ChartBackend};

    /// Cell 2 (feature ON + config Legacy) — always Legacy.
    #[test]
    fn resolve_legacy_always_returns_legacy() {
        assert_eq!(resolve_backend(ChartBackend::Legacy), ChartBackend::Legacy);
    }

    /// Cell 1 (feature ON + config New) — stays New.
    #[cfg(feature = "session_chart")]
    #[test]
    fn resolve_new_with_feature_stays_new() {
        assert_eq!(resolve_backend(ChartBackend::New), ChartBackend::New);
    }

    /// Cell 3 (feature OFF + config New) — falls back to Legacy.
    #[cfg(not(feature = "session_chart"))]
    #[test]
    fn resolve_new_without_feature_falls_back_to_legacy() {
        super::reset_backend_fallback_warned();
        assert_eq!(resolve_backend(ChartBackend::New), ChartBackend::Legacy);
        // Second call is still Legacy — the latch only suppresses the
        // warning, not the fall-back itself.
        assert_eq!(resolve_backend(ChartBackend::New), ChartBackend::Legacy);
    }

    /// Cell 4 (feature OFF + config Legacy) — Legacy.
    #[cfg(not(feature = "session_chart"))]
    #[test]
    fn resolve_legacy_without_feature_is_legacy() {
        assert_eq!(resolve_backend(ChartBackend::Legacy), ChartBackend::Legacy);
    }
}
