//! # session_chart_panel — pane-grid host for [`crate::session_chart::SessionChart`]
//!
//! Slice F2 of the multi-window plan retired the standalone-window
//! `session_chart_window.rs` and folded its per-instance state into a
//! regular `pane_grid` cell keyed by [`midas_core::SessionChartId`].
//! Functionally identical to the old `SessionChartWindow`: holds the
//! widget (behind `Arc<RwLock<_>>` so the shader `Program` can borrow
//! it from `view()`), the driver Arc (lifeline for the pump task),
//! and the originating [`crate::session_chart::SessionChartRequest`].
//!
//! The view function returns `Element<'_, Message>` for any message
//! routing path — the pane grid embeds the result like any other
//! panel body.

#![cfg(feature = "session_chart")]

use std::sync::Arc;

use iced::widget::{button, column, container, row, shader, stack, text};
use iced::{Alignment, Element, Length};
use midas_core::SessionChartId;
use parking_lot::RwLock;

use crate::session_chart::{
    SessionChart, SessionChartDriver, SessionChartProgram, SessionChartRequest,
};

/// Per-pane state for a session-chart panel. Mirrors the fields the
/// retired `SessionChartWindow` held, minus the OS `window::Id` —
/// session-chart panes now live as pane-grid cells in regular
/// multi-window `WindowState`s.
pub struct SessionChartPanel {
    /// The widget value, shared with the `SessionChartProgram` each
    /// frame through a cheap `Arc::clone`. The app's message
    /// handlers take `write()` guards; paint takes `try_write()`.
    pub widget: Arc<RwLock<SessionChart>>,
    /// Driver Arc — dropping this aborts the pump task via
    /// `JoinHandle::Drop`. Held for the lifetime of the panel.
    #[allow(dead_code)]
    pub driver: Arc<SessionChartDriver>,
    /// The request that spawned this panel. Used to title the pane
    /// title bar and to re-subscribe on EhPolicy changes (future
    /// slice).
    pub request: SessionChartRequest,
}

impl SessionChartPanel {
    /// Build a fresh per-pane state. Wraps the widget in an
    /// `Arc<RwLock<_>>` for sharing with the shader program.
    pub fn new(
        widget: SessionChart,
        driver: Arc<SessionChartDriver>,
        request: SessionChartRequest,
    ) -> Self {
        Self {
            widget: Arc::new(RwLock::new(widget)),
            driver,
            request,
        }
    }

    /// Human-readable title for the pane title bar.
    pub fn title(&self) -> String {
        format!(
            "Session · {} · {}",
            self.request.ticker,
            period_label(self.request.period),
        )
    }

    /// Build the pane body element.
    ///
    /// Layout matches the retired window's chrome — a small overlay
    /// with status / EH / tool buttons rendered on top of the GPU
    /// shader surface.
    pub fn view(&self, panel_id: SessionChartId) -> Element<'_, crate::app::Message> {
        // -- Status line --------------------------------------------
        let (axis_kind, eh_policy) = {
            let g = self.widget.read();
            (g.axis_kind(), g.eh_policy())
        };
        let series_arc = { self.widget.read().series() };
        let series_len = {
            let g = series_arc.read();
            g.len()
        };
        let series_version = { series_arc.read().version() };

        let header = text(format!(
            "{} · {} · axis={} · period={} · eh={} · len={} · v{}",
            self.request.ticker,
            self.request.calendar_id.0,
            axis_kind_label(axis_kind),
            period_label(self.request.period),
            eh_policy.short_label(),
            series_len,
            series_version,
        ))
        .size(12);

        let eh_btn = button(text(format!("EH: {}", eh_policy.short_label())).size(11))
            .on_press(crate::app::Message::SessionChartCyclePolicy(panel_id))
            .padding([2, 8]);

        let level_active = { self.widget.read().is_level_tool_active() };
        let add_level_btn =
            button(text(if level_active { "Level *" } else { "Add Level" }).size(11))
                .on_press(crate::app::Message::SessionChartToggleLevelTool(panel_id))
                .padding([2, 8]);

        let bracket_mode = {
            let g = self.widget.read();
            if g.is_bracket_tool_active() {
                g.bracket_tool_mode()
            } else {
                None
            }
        };
        let buy_btn_label = match bracket_mode {
            Some(midas_scene::tools::BracketToolMode::AwaitingEntry {
                side: midas_scene::tools::Side::Long,
            })
            | Some(midas_scene::tools::BracketToolMode::AwaitingTarget {
                side: midas_scene::tools::Side::Long,
                ..
            })
            | Some(midas_scene::tools::BracketToolMode::AwaitingStop {
                side: midas_scene::tools::Side::Long,
                ..
            }) => "Buy *",
            _ => "Buy Bracket",
        };
        let sell_btn_label = match bracket_mode {
            Some(midas_scene::tools::BracketToolMode::AwaitingEntry {
                side: midas_scene::tools::Side::Short,
            })
            | Some(midas_scene::tools::BracketToolMode::AwaitingTarget {
                side: midas_scene::tools::Side::Short,
                ..
            })
            | Some(midas_scene::tools::BracketToolMode::AwaitingStop {
                side: midas_scene::tools::Side::Short,
                ..
            }) => "Sell *",
            _ => "Sell Bracket",
        };
        let buy_bracket_btn = button(text(buy_btn_label).size(11))
            .on_press(crate::app::Message::SessionChartActivateBuyBracketTool(
                panel_id,
            ))
            .padding([2, 8]);
        let sell_bracket_btn = button(text(sell_btn_label).size(11))
            .on_press(crate::app::Message::SessionChartActivateSellBracketTool(
                panel_id,
            ))
            .padding([2, 8]);

        // Slice F2: no per-pane "Close" button — pane-grid title bar
        // close button (added by the standard pane chrome) handles
        // tear-down. Removing the redundant close button matches the
        // chart / watchlist / order panel chrome.
        let overlay = container(
            row![
                header,
                eh_btn,
                add_level_btn,
                buy_bracket_btn,
                sell_bracket_btn,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(6)
        .width(Length::Fill);

        // -- GPU shader surface ------------------------------------
        let program: SessionChartProgram<crate::app::Message> =
            SessionChartProgram::new(Arc::clone(&self.widget));
        let canvas: Element<'_, crate::app::Message> = shader(program)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // Compose: shader at the bottom, overlay on top. iced's
        // `stack![]` paints back-to-front. The overlay sits in a
        // `column![overlay]` so it visually hangs off the top edge
        // without consuming all vertical space (see
        // feedback_iced_fill_height.md — `Fill` inside a Column would
        // collapse the shader to zero height).
        let chrome: Element<'_, crate::app::Message> = column![overlay].spacing(0).into();

        stack![canvas, chrome].into()
    }
}

fn axis_kind_label(kind: crate::session_chart::AxisKind) -> &'static str {
    match kind {
        crate::session_chart::AxisKind::Continuous => "Continuous",
        crate::session_chart::AxisKind::Compressed => "Compressed",
        crate::session_chart::AxisKind::SessionIndex => "SessionIndex",
    }
}

fn period_label(period: midas_bars::BarPeriod) -> &'static str {
    use midas_bars::{BarPeriod, CalendarSpan, ClockInterval, SessionSpan};
    match period {
        BarPeriod::Clock(ClockInterval::Seconds(5)) => "S5",
        BarPeriod::Clock(ClockInterval::Seconds(_)) => "Sn",
        BarPeriod::Clock(ClockInterval::Minutes(1)) => "M1",
        BarPeriod::Clock(ClockInterval::Minutes(5)) => "M5",
        BarPeriod::Clock(ClockInterval::Minutes(15)) => "M15",
        BarPeriod::Clock(ClockInterval::Minutes(_)) => "Mn",
        BarPeriod::Clock(ClockInterval::Hours(1)) => "H1",
        BarPeriod::Clock(ClockInterval::Hours(_)) => "Hn",
        BarPeriod::Clock(_) => "?",
        BarPeriod::Session(SessionSpan::Regular) => "D1·RTH",
        BarPeriod::Session(SessionSpan::Extended) => "D1·ETH",
        BarPeriod::Session(SessionSpan::Eth) => "D1·ETH",
        BarPeriod::Session(_) => "Sess?",
        BarPeriod::Calendar(CalendarSpan::Week) => "W1",
        BarPeriod::Calendar(CalendarSpan::Month) => "MN1",
        BarPeriod::Calendar(CalendarSpan::Quarter) => "Q1",
        BarPeriod::Calendar(CalendarSpan::Year) => "Y1",
        BarPeriod::Calendar(_) => "Cal?",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_kind_label_covers_all_variants() {
        assert_eq!(
            axis_kind_label(crate::session_chart::AxisKind::Continuous),
            "Continuous"
        );
        assert_eq!(
            axis_kind_label(crate::session_chart::AxisKind::Compressed),
            "Compressed"
        );
        assert_eq!(
            axis_kind_label(crate::session_chart::AxisKind::SessionIndex),
            "SessionIndex"
        );
    }

    #[test]
    fn period_label_covers_canonical_periods() {
        use midas_bars::BarPeriod;
        assert_eq!(period_label(BarPeriod::m1()), "M1");
        assert_eq!(period_label(BarPeriod::m5()), "M5");
        assert_eq!(period_label(BarPeriod::h1()), "H1");
        assert_eq!(period_label(BarPeriod::d1_rth()), "D1·RTH");
        assert_eq!(period_label(BarPeriod::w1()), "W1");
        assert_eq!(period_label(BarPeriod::mn1()), "MN1");
    }
}
