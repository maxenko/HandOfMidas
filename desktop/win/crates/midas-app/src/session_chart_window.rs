//! # session_chart_window — Phase D host plumbing for
//! [`crate::session_chart::SessionChart`]
//!
//! Feature-gated on `session_chart`. Holds per-window state for every
//! `window::open()`-hosted session chart: the widget (behind an
//! `Arc<RwLock<_>>` so the shader `Program` can borrow it from the
//! `view()` path), the driver Arc (lifeline for the pump task), and
//! the originating [`crate::session_chart::SessionChartRequest`].
//!
//! ## View
//!
//! The window now renders an iced `shader(SessionChartProgram)`
//! widget that paints candles, grid, session bands, separators, and
//! crosshair via the GPU through
//! [`midas_render::ChartRenderer`]. A small overlay chrome on top
//! shows the status line + EH-cycle button + Close button.
//!
//! ## Known TODOs
//!
//! - **Text rendering** (badge labels, axis priceline numbers, tooltip
//!   text). The legacy SDF badge + cryoglyph glyph pipelines are not
//!   yet wired through the `RenderBuckets` bridge — see
//!   `plan/session-aware-charts/00b-integration-strategy.md` "S9" and
//!   `plan/session-aware-charts/00a-ideal-design.md` "[R2-G-2]".
//! - **Pan / zoom interaction** beyond the stubbed wheel-cycles-EH
//!   behaviour in `shader.rs::update`. Real pan via
//!   `set_time_window`, zoom via `set_price_range`, drag via an
//!   interaction state machine are a follow-up slice.
//! - **Keyboard shortcuts** (arrow-keys pan, +/- zoom, "E" cycles
//!   EhPolicy, "X" closes window). The "EH" button cycles, the
//!   "Close" button closes, everything else is TODO.
//! - **Period picker drop-down** — widget accepts any `BarPeriod`
//!   the calendar validates; the UI affordance lands with the
//!   drop-down polish slice.
//!
//! ## Manual smoke test
//!
//! ```bash
//! cargo run -p midas-app --features session_chart
//! ```
//!
//! - Click "Session chart — BTC M1" in the toolbar.
//! - A new window opens titled "Session · BTC-USD · M1".
//! - Expect to see candles rendering with a grid, session bands on
//!   XNYS charts, and a crosshair following the mouse.
//! - Known limitation: no text inside badges, no axis labels. These
//!   will appear when the glyph pipeline is wired through
//!   (`ChartScene::labels` / `axis_labels` are currently empty by
//!   design — see `gpu_renderer.rs` deferred-gap docs).

#![cfg(feature = "session_chart")]

use std::sync::Arc;

use iced::widget::{button, column, container, row, shader, stack, text};
use iced::{window, Alignment, Element, Length};
use parking_lot::RwLock;

use crate::session_chart::{
    SessionChart, SessionChartDriver, SessionChartProgram, SessionChartRequest,
};

/// Per-window state for a standalone session-chart window.
///
/// The widget lives behind an `Arc<RwLock<_>>` so the shader
/// `Program` built in `view()` can take a short-lived write guard to
/// run `paint_buckets` without requiring `&mut self` on the window
/// state. The lock is `try_write()`-only on the paint path (see
/// `shader.rs`) — paint never stalls on the app's update pump.
pub struct SessionChartWindow {
    /// The widget value, shared with the `SessionChartProgram` each
    /// frame through a cheap `Arc::clone`. The app's message
    /// handlers take `write()` guards; paint takes `try_write()`.
    pub widget: Arc<RwLock<SessionChart>>,
    /// Driver Arc — dropping this aborts the pump task via
    /// `JoinHandle::Drop`. Held for the lifetime of the window.
    #[allow(dead_code)]
    pub driver: Arc<SessionChartDriver>,
    /// The request that spawned this window. Used to title the
    /// window and to re-subscribe on EhPolicy changes (future slice).
    pub request: SessionChartRequest,
}

impl SessionChartWindow {
    /// Build a fresh per-window state. Wraps the widget in an
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

    /// Human-readable title for the OS window. Consumed by future
    /// per-window `title(window_id)` wiring — iced 0.14's
    /// `daemon().title(fn)` path currently returns the single
    /// "Hand of Midas" string for every window; Phase D leaves the
    /// per-window title wiring as a TODO (low-risk, UX polish).
    #[allow(dead_code)]
    pub fn title(&self) -> String {
        format!(
            "Session · {} · {}",
            self.request.ticker,
            period_label(self.request.period),
        )
    }

    /// Build the iced element rendered inside this window.
    ///
    /// Layout:
    ///
    /// ```text
    /// ┌──────────────────────────────────────────────┐
    /// │ status line · [EH] [Close]                   │  <- overlay
    /// ├──────────────────────────────────────────────┤
    /// │                                              │
    /// │   shader(SessionChartProgram) fills          │
    /// │   the rest of the window                     │
    /// │                                              │
    /// └──────────────────────────────────────────────┘
    /// ```
    ///
    /// The shader widget renders candles, grid, session bands,
    /// separators, and crosshair via the GPU. The overlay chrome is
    /// rendered on top via `iced::widget::stack![]`.
    pub fn view(&self, window_id: window::Id) -> Element<'_, crate::app::Message> {
        // -- Status line --------------------------------------------
        //
        // Read-only: short-lived `read()` guard so we can format the
        // header without blocking the GPU paint. The guard never
        // crosses an await (iced's view fn is synchronous).
        let (axis_kind, eh_policy) = {
            let g = self.widget.read();
            (g.axis_kind(), g.eh_policy())
        };
        let series_arc = { self.widget.read().series() };
        let series_len = {
            let g = series_arc.read();
            g.len()
        };
        // Chart engine version counter — bumps every time a tick
        // arrives. Cheap debug aid.
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
            .on_press(crate::app::Message::SessionChartCyclePolicy(window_id))
            .padding([2, 8]);

        // Slice 4 chart-transition: "Add Level" toolbar button.
        // Toggles the level-placement tool on the `SessionChart`
        // widget. The app-side message handler flips the tool state.
        let level_active = { self.widget.read().is_level_tool_active() };
        let add_level_btn =
            button(text(if level_active { "Level *" } else { "Add Level" }).size(11))
                .on_press(crate::app::Message::SessionChartToggleLevelTool(window_id))
                .padding([2, 8]);

        // Slice 5b chart-transition: "Buy Bracket" / "Sell Bracket"
        // toolbar chips activate the `BracketTool` on the widget for
        // Long / Short placement respectively. The app-side message
        // handler flips the tool state + translates the resulting
        // effects to `TickerMsg`s via the draft-then-save sequence.
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
                window_id,
            ))
            .padding([2, 8]);
        let sell_bracket_btn = button(text(sell_btn_label).size(11))
            .on_press(crate::app::Message::SessionChartActivateSellBracketTool(
                window_id,
            ))
            .padding([2, 8]);

        let close_btn = button(text("Close").size(11))
            .on_press(crate::app::Message::WindowCloseRequested(window_id))
            .padding([2, 8]);

        let overlay = container(
            row![
                header,
                eh_btn,
                add_level_btn,
                buy_bracket_btn,
                sell_bracket_btn,
                close_btn,
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(6)
        .width(Length::Fill);

        // -- GPU shader surface ------------------------------------
        //
        // A fresh `SessionChartProgram` is built each frame — it's a
        // thin struct holding an `Arc::clone()` of the widget Arc, so
        // construction cost is one ref-count bump.
        let program: SessionChartProgram<crate::app::Message> =
            SessionChartProgram::new(Arc::clone(&self.widget));
        let canvas: Element<'_, crate::app::Message> = shader(program)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // -- Compose: shader at the bottom, overlay on top ---------
        //
        // `iced::widget::stack![]` paints children back-to-front —
        // the shader first, then the overlay chrome on top. The
        // overlay sits in a `column![ overlay, Rule::horizontal ]` so
        // it visually hangs off the top edge without consuming all
        // vertical space (feedback_iced_fill_height.md — `Fill`
        // inside a Column would collapse the shader to zero height).
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
        // `ClockInterval` is `#[non_exhaustive]`; any future family
        // (e.g. `Days(u32)`) falls through with a neutral label until the
        // UI learns to format it.
        BarPeriod::Clock(_) => "?",
        BarPeriod::Session(SessionSpan::Regular) => "D1·RTH",
        BarPeriod::Session(SessionSpan::Extended) => "D1·ETH",
        BarPeriod::Session(SessionSpan::Eth) => "D1·ETH",
        // `SessionSpan` is `#[non_exhaustive]`; future variants fall
        // through with a neutral label.
        BarPeriod::Session(_) => "Sess?",
        BarPeriod::Calendar(CalendarSpan::Week) => "W1",
        BarPeriod::Calendar(CalendarSpan::Month) => "MN1",
        BarPeriod::Calendar(CalendarSpan::Quarter) => "Q1",
        BarPeriod::Calendar(CalendarSpan::Year) => "Y1",
        // `CalendarSpan` is `#[non_exhaustive]`; future variants fall
        // through with a neutral label.
        BarPeriod::Calendar(_) => "Cal?",
        // `BarPeriod` itself is `#[non_exhaustive]`; any entirely-new
        // variant (e.g. `BarPeriod::Range(...)` for range bars) gets a
        // neutral placeholder until the UI adds explicit branches.
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
