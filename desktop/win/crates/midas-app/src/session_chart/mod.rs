//! # session_chart — Phase B (S8) + Phase C (S10–S14) implementation
//!
//! Feature-gated module that ships the session-aware chart widget
//! powered by the Phase-A stack: `midas-calendar` → `midas-bars` →
//! `midas-stream` → `midas-bars-adapter` → `midas-axis` →
//! `midas-scene`.
//!
//! ## Phase B (S8) — MVP vertical slice
//!
//! - Pipeline proven end-to-end on a crypto M1 chart with a
//!   `ContinuousAxis`.
//! - [`driver::SessionChartDriver`] pumps [`midas_stream::BarStream`]
//!   output into a shared [`midas_bars::CandleSeries`]; a
//!   `tokio::sync::watch` version counter drives paint coalescing.
//! - [`scene_builder::build_scene`] assembles a
//!   [`midas_scene::ChartScene`] from the runtime state.
//! - [`primitives_bridge::translate`] converts
//!   [`midas_scene::ScenePrimitives`] into the legacy
//!   [`midas_chart::instances`] GPU-layout types so the existing
//!   `midas-render` pipelines can draw.
//! - [`widget::SessionChart`] is the composition root.
//!
//! ## Phase C (S10–S14) — horizontal expansion
//!
//! - **S10: XNYS intraday support** — [`widget::SessionChart`] is now
//!   calendar-agnostic. Construction via
//!   `calendar.time_axis_policy()` picks either a `ContinuousAxis`
//!   (crypto) or a `CompressedAxis` (XNYS / any future exchange).
//!   [`widget::AxisKind`] enumerates the supported axis shapes.
//! - **S11: `BarPeriod::Session(Regular)`** — handled via the existing
//!   `midas_calendar::bar_window` path; the aggregator already emits
//!   session-scoped bars. The widget stores the period as a field
//!   and exposes [`widget::SessionChart::set_period`] for the host to
//!   reflect re-subscribes.
//! - **S12: `BarPeriod::Calendar(Week|Month)`** — same mechanism. No
//!   aggregator / adapter changes needed; the calendar computes the
//!   bar window.
//! - **S13: Holiday markers** — [`scene_builder::SceneLayers`] now
//!   carries a `holidays` flag. [`scene_builder::SceneLayers::from_eh_policy`]
//!   gates it on for XNYS, off for crypto.
//! - **S14: EhPolicy UI toggle** — [`widget::EhPolicy`] with
//!   `ShowAll` / `HideExtended` / `ShowBarsOnly`. The widget exposes
//!   [`widget::SessionChart::cycle_eh_policy`] for a cycling chip in
//!   the host view. Re-subscription through
//!   [`midas_stream::Filtered<_, EhFilter>`] is the host's concern —
//!   the handler in `app.rs` composes the stream before handing it
//!   to [`driver::SessionChartDriver::spawn`].
//!
//! ## Historical / live seam — known MVP limitation
//!
//! `midas_bars_adapter::subscribe_aggregated_bars` drives the
//! aggregator off the broker-core tick stream
//! (`source.subscribe_ticks(...)`). This works for every supported
//! `BarPeriod` in Phase C — Clock, Session, and Calendar windows
//! all roll over correctly when the tick timestamp crosses the
//! calendar's `bar_window`. **However**, the current adapter path
//! does NOT preload historical bars on subscribe; long-period charts
//! (`Session(Regular)`, `Calendar(Week)`, etc.) appear empty until a
//! live tick arrives and, for a D1-RTH chart, don't show a bar until
//! one full session's worth of ticks has elapsed. `subscribe_realtime_bars`
//! would emit 5-sec bars which the current aggregator consumes as
//! if they were ticks; that path is still wired for intraday
//! (S10 / M1, M5, etc.). A historical-bars backfill (via
//! `MarketDataSource::historical_bars` + a
//! [`midas_stream::HistoryThenLive`] combinator) is scheduled for
//! the Phase D migration — we accept ticks-only feed here.
//!
//! ## Integration with `midas-app`
//!
//! - [`crate::app::Message::OpenSessionChart`] — extended in Phase C
//!   to carry `(ticker, BarPeriod, CalendarId)` via
//!   [`SessionChartRequest`]. The handler in `app.rs` spawns a
//!   driver, opens a standalone iced window, and stores the widget
//!   inside `floating_session_charts` keyed by the returned
//!   `window::Id`.
//! - The toolbar gains three feature-gated buttons:
//!   "Session chart — BTC M1", "Session chart — AAPL M5",
//!   "Session chart — SPY D1 RTH" — each sends a pre-filled
//!   [`SessionChartRequest`].
//! - The standalone window renders a diagnostic text view of the
//!   chart state (series length, axis kind, EH policy, period, a
//!   textual summary of the most-recent candles). This fulfils the
//!   Phase C "open an actual iced window" deliverable without
//!   coupling to the legacy `wgpu` chart pipeline — which Phase D
//!   retires wholesale. The window IS actively painting whenever the
//!   driver's `version` watch ticks. Full GPU-rendered paint
//!   [`paint_buckets`](widget::SessionChart::paint_buckets) is
//!   unit-tested in-place; wiring it through an iced `shader::Program`
//!   is Phase D work.
//!
//! ## Non-goals (by design)
//!
//! - No mutation of the legacy `ChartPanel` / `ChartScene` /
//!   `Camera2D` state. Phase C is still purely additive.
//! - No GPU-feature parity with the legacy chart in the standalone
//!   window. The window renders a deterministic textual projection;
//!   Phase D replaces this with the full `shader::Program` once the
//!   legacy chart is retired.
//! - No keyboard shortcuts yet. Arrow-keys pan, +/- zoom, "E" cycles
//!   `EhPolicy`, "X" closes the window — documented in
//!   `widget::SessionChart` module docs; wiring is a post-Phase-C
//!   UX polish slice.
//! - No disk persistence of the per-chart state. G-4 in
//!   `00a-ideal-design.md` — scheduled post-Phase-C.
//!
//! ## Testing strategy
//!
//! - Widget / scene-builder — unit tests on axis kind, EhPolicy
//!   cycling, layer derivation, period reflection.
//! - Driver — deterministic mock stream; version counter progression.
//! - E2E — `tests/session_chart_e2e.rs` exercises the full crypto M1
//!   pipeline end-to-end. Phase C adds an XNYS M1 pipeline test and
//!   an EhPolicy toggle test.

#![cfg(feature = "session_chart")]
#![allow(dead_code, unused_imports)]

pub mod axis_box;
pub mod driver;
pub mod gpu_renderer;
pub mod policy;
pub mod primitives_bridge;
pub mod registry;
pub mod scene_builder;
pub mod shader;
pub mod widget;

pub use axis_box::AxisKind;
pub use driver::{DriverError, SessionChartDriver, VersionReceiver};
pub use gpu_renderer::SessionChartRenderer;
pub use policy::EhPolicy;
pub use primitives_bridge::{translate, BadgeMetaInstance, RenderBuckets, TextMetaInstance};
pub use registry::SymbolSeriesRegistry;
pub use scene_builder::{build_scene, SceneConfig, SceneLayers};
pub use shader::{
    session_chart_shader, SessionChartPipeline, SessionChartPrimitive, SessionChartProgram,
    SessionChartWidgetState,
};

// NOTE: `SessionChartProgram<M>` is generic; binary-target callers in
// `session_chart_window.rs` parameterize with `crate::app::Message`
// while the library target (integration tests) can parameterize with
// any Message type they define.
pub use widget::{SessionChart, SessionChartError};

use midas_bars::BarPeriod;
use midas_calendar::CalendarId;

/// Payload for [`crate::app::Message::OpenSessionChart`]. Phase C
/// extended the original unit-variant to a structured request so the
/// handler can route to the right symbol / period / calendar without
/// hard-coding the BTC M1 vertical slice.
///
/// Constructors:
///
/// - [`SessionChartRequest::btc_m1`] — the Phase B baseline.
/// - [`SessionChartRequest::aapl_m5`] — XNYS intraday.
/// - [`SessionChartRequest::spy_d1_rth`] — XNYS daily RTH.
#[derive(Debug, Clone)]
pub struct SessionChartRequest {
    pub ticker: String,
    pub period: BarPeriod,
    /// Calendar id the ticker resolves to. Set explicitly so the
    /// caller can disambiguate without a resolver round-trip; the
    /// handler validates the actual resolver's answer matches.
    pub calendar_id: CalendarId,
}

impl SessionChartRequest {
    /// Baseline Phase B request — BTC-USD on crypto, M1 clock bars.
    pub fn btc_m1() -> Self {
        Self {
            ticker: "BTC-USD".into(),
            period: BarPeriod::m1(),
            calendar_id: midas_calendar::CRYPTO_SPOT_ID,
        }
    }

    /// XNYS intraday — AAPL M5 clock bars.
    pub fn aapl_m5() -> Self {
        Self {
            ticker: "AAPL".into(),
            period: BarPeriod::m5(),
            calendar_id: midas_calendar::XNYS_ID,
        }
    }

    /// XNYS daily RTH — SPY `BarPeriod::Session(Regular)`.
    pub fn spy_d1_rth() -> Self {
        Self {
            ticker: "SPY".into(),
            period: BarPeriod::d1_rth(),
            calendar_id: midas_calendar::XNYS_ID,
        }
    }
}
