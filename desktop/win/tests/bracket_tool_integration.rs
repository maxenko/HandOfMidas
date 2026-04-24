//! Slice 5b of chart-transition: bracket-tool integration tests.
//!
//! Each test exercises the full `SessionChart` → bracket `ToolEffect`
//! → `ProjectedEffect` → projected `TickerMsg` round-trip, WITHOUT
//! spinning up the iced event loop. The tests drive the widget
//! directly via `handle_bracket_input` and assert on the drained
//! `ProjectedEffect`s — the data carrier for app-level TickerMsg
//! translation.
//!
//! Gated on `session_chart_tests` (alongside `level_end_to_end.rs`) so
//! the default test invocation doesn't pay the extra build cost.

#![cfg(feature = "session_chart_tests")]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::TimeZone;
use midas_app::session_chart::{ProjectedEffect, SessionChart, SessionChartDriver};
use midas_axis::{PriceRange, Viewport};
use midas_bars::{BarPeriod, CandleSeries, Symbol};
use midas_calendar::{crypto_spot, Timestamp};
use midas_scene::input::{InputEvent, Modifiers, MouseButton, Point};
use midas_scene::tools::{BracketToolMode, LegRole, Side as BracketSide};
use midas_scene::ThemePalette;
use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
use parking_lot::RwLock;
use tokio::sync::mpsc;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

/// Mock bar stream that never emits.
struct EmptyBarStream {
    meta: BarStreamMeta,
    rx: mpsc::Receiver<midas_bars::Candle>,
}

impl EmptyBarStream {
    fn btc_m1() -> (mpsc::Sender<midas_bars::Candle>, Self) {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let (tx, rx) = mpsc::channel(1);
        let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
        (tx, Self { meta, rx })
    }
}

#[async_trait]
impl BarStream for EmptyBarStream {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }
    async fn next(&mut self) -> Option<midas_bars::Candle> {
        self.rx.recv().await
    }
    async fn snapshot(
        &mut self,
        _range: TimeRange,
    ) -> Result<Vec<midas_bars::Candle>, StreamError> {
        Err(StreamError::NotSeekable)
    }
}

fn fresh_btc_series() -> Arc<RwLock<CandleSeries>> {
    let cal = crypto_spot();
    let sym = Symbol::new("BTC-USD", cal.id());
    Arc::new(RwLock::new(CandleSeries::new(
        cal.id(),
        BarPeriod::m1(),
        sym,
    )))
}

fn mk_widget() -> SessionChart {
    let (_tx, stream) = EmptyBarStream::btc_m1();
    let driver = Arc::new(SessionChartDriver::spawn(fresh_btc_series(), stream));
    let cal = crypto_spot();
    let start = utc(2024, 3, 1, 0, 0);
    let end = utc(2024, 3, 2, 0, 0);
    let pr = PriceRange::new(49_900.0, 50_200.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    SessionChart::new(
        driver,
        cal,
        BarPeriod::m1(),
        pr,
        vp,
        ThemePalette::dark_default(),
        (start, end),
    )
    .expect("widget construction succeeds on canonical inputs")
}

fn left_click(y: f32) -> InputEvent {
    InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(500.0, y),
        modifiers: Modifiers::default(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_click_long_places_bracket_via_projected_effects() {
    // Activate tool → 3 left-clicks at entry / TP / SL prices →
    // drain projected effects → assert sequence.
    let mut widget = mk_widget();
    widget.activate_buy_bracket_tool();
    assert!(widget.is_bracket_tool_active());
    assert_eq!(
        widget.bracket_tool_mode(),
        Some(BracketToolMode::AwaitingEntry {
            side: midas_scene::tools::Side::Long,
        })
    );

    // Click 1: entry at 50_000.
    widget.update_bracket_preview(50_000.0, 200.0);
    widget.handle_bracket_input(left_click(200.0));
    // Click 2: TP at 50_100.
    widget.update_bracket_preview(50_100.0, 150.0);
    widget.handle_bracket_input(left_click(150.0));
    // Click 3: SL at 49_950.
    widget.update_bracket_preview(49_950.0, 250.0);
    widget.handle_bracket_input(left_click(250.0));

    let effects = widget.drain_level_effects();
    assert_eq!(effects.len(), 5);
    assert_eq!(
        effects[0],
        ProjectedEffect::BeginDraftBracket {
            side: BracketSide::Long,
            entry: 50_000.0,
        }
    );
    assert_eq!(
        effects[1],
        ProjectedEffect::SetDraftLeg {
            role: LegRole::Entry,
            price: 50_000.0,
        }
    );
    assert_eq!(
        effects[2],
        ProjectedEffect::SetDraftLeg {
            role: LegRole::Tp,
            price: 50_100.0,
        }
    );
    assert_eq!(
        effects[3],
        ProjectedEffect::SetDraftLeg {
            role: LegRole::Sl,
            price: 49_950.0,
        }
    );
    assert_eq!(effects[4], ProjectedEffect::CommitDraftBracket);
    // Tool reset to AwaitingEntry for next bracket (multi-bracket flow).
    assert_eq!(
        widget.bracket_tool_mode(),
        Some(BracketToolMode::AwaitingEntry {
            side: midas_scene::tools::Side::Long,
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_click_short_places_bracket() {
    let mut widget = mk_widget();
    widget.activate_sell_bracket_tool();
    widget.update_bracket_preview(50_000.0, 200.0);
    widget.handle_bracket_input(left_click(200.0));
    widget.update_bracket_preview(49_900.0, 250.0);
    widget.handle_bracket_input(left_click(250.0));
    widget.update_bracket_preview(50_050.0, 150.0);
    widget.handle_bracket_input(left_click(150.0));

    let effects = widget.drain_level_effects();
    assert!(!effects.is_empty());
    assert_eq!(
        effects[0],
        ProjectedEffect::BeginDraftBracket {
            side: BracketSide::Short,
            entry: 50_000.0,
        }
    );
    assert_eq!(
        *effects.last().unwrap(),
        ProjectedEffect::CommitDraftBracket
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mid_placement_deactivate_emits_cancel_draft() {
    // R11: deactivating mid-placement must emit CancelDraftBracket so
    // the host translates to TickerMsg::CancelBracket.
    let mut widget = mk_widget();
    widget.activate_buy_bracket_tool();
    widget.update_bracket_preview(50_000.0, 200.0);
    widget.handle_bracket_input(left_click(200.0));
    // Drain the pending begin-draft effects.
    let _ = widget.drain_level_effects();
    // Now in AwaitingTarget. Deactivate → must emit cancel.
    widget.deactivate_bracket_tool();
    let effs = widget.drain_level_effects();
    assert!(effs.contains(&ProjectedEffect::CancelDraftBracket));
    // Tool gone.
    assert!(!widget.is_bracket_tool_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn escape_during_placement_cancels_via_scene_handler() {
    // Escape → BracketTool emits CancelDraftBracket → projected →
    // translates to TickerMsg::CancelBracket at the host.
    let mut widget = mk_widget();
    widget.activate_buy_bracket_tool();
    widget.update_bracket_preview(50_000.0, 200.0);
    widget.handle_bracket_input(left_click(200.0));
    // Drain pending effects.
    let _ = widget.drain_level_effects();
    // Escape.
    widget.handle_bracket_input(InputEvent::KeyDown {
        key: midas_scene::input::Key::Escape,
        modifiers: Modifiers::default(),
    });
    let effs = widget.drain_level_effects();
    assert!(effs.contains(&ProjectedEffect::CancelDraftBracket));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activate_toggle_with_same_side_still_placing() {
    // Activate Buy, then Buy again — toolbar handler deactivates. We
    // simulate the app-side toggle here by just calling deactivate.
    let mut widget = mk_widget();
    widget.activate_buy_bracket_tool();
    assert!(widget.is_bracket_tool_active());
    widget.deactivate_bracket_tool();
    assert!(!widget.is_bracket_tool_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn amber_fires_on_wrong_side_placement() {
    // Long SL priced ABOVE entry = wrong-side. The classifier at the
    // layer level returns true; this test verifies the FSM still
    // completes (no enforcement) and the wrong-side status is
    // observable on the drained effects' leg prices.
    let mut widget = mk_widget();
    widget.activate_buy_bracket_tool();
    widget.update_bracket_preview(50_000.0, 200.0);
    widget.handle_bracket_input(left_click(200.0));
    widget.update_bracket_preview(50_100.0, 150.0);
    widget.handle_bracket_input(left_click(150.0));
    // SL at 50_050 — ABOVE entry (wrong side for Long).
    widget.update_bracket_preview(50_050.0, 170.0);
    widget.handle_bracket_input(left_click(170.0));

    let effects = widget.drain_level_effects();
    // SL leg effect at index 3.
    assert!(matches!(
        effects[3],
        ProjectedEffect::SetDraftLeg {
            role: LegRole::Sl,
            price,
        } if (price - 50_050.0).abs() < 1e-6
    ));
    // Wrong-side classifier says yes.
    assert!(midas_scene::tools::is_leg_on_wrong_side(
        midas_scene::tools::Side::Long,
        50_000.0,
        50_050.0,
        midas_scene::tools::LegKind::Sl,
    ));
}
