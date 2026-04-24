//! [`SessionChart`] — the iced-compatible widget scaffold.
//!
//! ## Phase C scope (S10–S14)
//!
//! Widens the S8 vertical-slice MVP from "crypto-only, ContinuousAxis,
//! M1 only" to a **calendar-agnostic**, **multi-period**, **EhPolicy-
//! aware** widget that the session-aware-charts plan calls for.
//!
//! Highlights:
//!
//! - [`AxisKind`] enumerates the three supported axis shapes:
//!   - `Continuous` — crypto / anything whose calendar reports
//!     [`TimeAxisPolicy::Continuous`][midas_calendar::TimeAxisPolicy].
//!   - `Compressed` — equities / futures / FX
//!     ([`TimeAxisPolicy::CompressedSessionBoundaries`]).
//!   - `SessionIndex` — analytical / extreme-zoom (reserved; the Phase
//!     C widget never constructs this, but the enum carves the slot so
//!     Phase F's auto-switch lands without a breaking change).
//! - [`EhPolicy`] — per the ideal design:
//!   - `ShowAll` — pre+RTH+post candles + bands + separators + full
//!     chrome.
//!   - `HideExtended` — ethicksmarket bars filtered OUT via
//!     [`midas_stream::Filtered<_, EhFilter>`] at the stream seam;
//!     the scene chrome stays identical (band/separator layers paint
//!     only *visible* sessions, and nothing in Extended windows is
//!     visible by construction).
//!   - `ShowBarsOnly` — bars render (tinted by session kind from the
//!     candle's `SessionKind`) but bands + separators don't emit.
//! - Period picker — the widget carries a [`BarPeriod`][midas_bars::BarPeriod]
//!   field. Re-subscription lives on the host (the handler in `app.rs`
//!   owns the driver lifetime); the widget exposes
//!   [`SessionChart::set_period`] for the host to reflect the new
//!   period after a re-subscribe.
//!
//! ## State model
//!
//! The widget is a **value** (not a pin-pointer-stable iced widget
//! node) — all large fields are `Arc`-shared or `Copy`. `series` is
//! an [`Arc<RwLock<CandleSeries>>`] shared with the driver — the pump
//! is the sole writer, the paint path is one of several concurrent
//! readers. Paint takes a short-lived `read()` guard inside
//! [`paint_buckets`](SessionChart::paint_buckets) and drops it before
//! returning; the guard never escapes the paint scope and never
//! crosses an `.await`.
//!
//! ## Interaction
//!
//! Event translation — mouse move updating `interaction.crosshair_px`,
//! wheel → zoom, drag → pan — is an input-layer concern. The widget
//! exposes direct mutators
//! ([`set_crosshair`](SessionChart::set_crosshair),
//! [`clear_crosshair`](SessionChart::clear_crosshair),
//! [`set_viewport`](SessionChart::set_viewport),
//! [`set_axis`](SessionChart::set_axis),
//! [`set_price_range`](SessionChart::set_price_range),
//! [`cycle_eh_policy`](SessionChart::cycle_eh_policy)) so host code +
//! integration tests can drive the state manually.
//!
//! ## Open TODOs deferred to later slices
//!
//! - Keyboard shortcuts (arrow-keys pan, +/- zoom, "E" cycles
//!   [`EhPolicy`], "X" close window). The iced window handler in
//!   `app.rs` wires the close action; the remaining bindings are UX
//!   polish scheduled post-Phase-C.
//! - On-screen toggle chip ("EH") — lives in the host window's view,
//!   not the widget value itself. The widget exposes
//!   [`SessionChart::eh_policy`] so the view can read it.
//! - Period cycling UX — the widget supports any [`BarPeriod`] the
//!   calendar accepts; the drop-down / cycling-button affordance is
//!   again a host-view concern.
//! - G-4 disk persistence of `(eh_policy, period, axis_kind)`.
//!   Deferred.

use std::sync::Arc;

use midas_axis::{AxisError, CompressedAxis, ContinuousAxis, PriceRange, Viewport};
use midas_bars::{BarPeriod, CandleSeries};
use midas_calendar::{ExchangeCalendar, Timestamp};
use midas_scene::{InteractionState, ScenePrimitives, SharedCandleSeries, ThemePalette};
use parking_lot::RwLock;

use super::axis_box::AxisBox;
use super::driver::SessionChartDriver;
use super::policy::EhPolicy;
use super::primitives_bridge::{translate, RenderBuckets};
use super::scene_builder::{build_scene, SceneConfig, SceneLayers};

pub use super::axis_box::AxisKind;

/// Error surface for [`SessionChart`] construction. App-harden M5:
/// previously the widget's axis construction `.expect`-panicked the UI
/// thread on degenerate inputs. Callers now handle the `Err` branch
/// and fall back gracefully (typically by closing the offending
/// window).
#[derive(Debug, thiserror::Error)]
pub enum SessionChartError {
    /// Axis construction rejected the inputs (degenerate time window,
    /// bad viewport width, etc.). Wraps the underlying
    /// [`AxisError`] for diagnostics.
    #[error("session chart axis construction failed: {0}")]
    Axis(#[from] AxisError),
}

/// Calendar-agnostic scaffold widget. Holds per-frame inputs for the
/// scene pipeline plus the shared series/driver handles.
pub struct SessionChart {
    /// Shared candle series written by the driver, read here through a
    /// short-lived read-guard during paint. Using the same
    /// `Arc<RwLock<_>>` the driver writes to — no deep-copy per frame
    /// (arch-audit F1 / R1 fix).
    series: SharedCandleSeries,
    /// Reference to the pump driver so dropping the widget cascades
    /// shutdown (driver's `Drop` aborts the pump task).
    #[allow(dead_code)]
    driver: Arc<SessionChartDriver>,
    /// The current time axis — sum over [`ContinuousAxis`] and
    /// [`CompressedAxis`].
    axis: AxisBox,
    /// Vertical price range shown in the viewport.
    price_range: PriceRange,
    /// Chart rectangle in logical pixels.
    viewport: Viewport,
    /// Colour palette.
    palette: ThemePalette,
    /// Calendar for session classification.
    calendar: &'static dyn ExchangeCalendar,
    /// Current bar period. Mutated via [`SessionChart::set_period`] by
    /// the host after a re-subscribe.
    period: BarPeriod,
    /// Per-chart interaction (hover / drag / crosshair / last-wheel).
    interaction: InteractionState,
    /// Extended-hours policy. Defaults to [`EhPolicy::ShowAll`].
    eh_policy: EhPolicy,
    /// Time range to warm the session-band + separator caches with.
    /// Typically derived from the axis; exposed so callers can pin a
    /// wider range for pre-load.
    time_window: (Timestamp, Timestamp),
    /// Reusable primitives buffer.
    primitives: ScenePrimitives,
    /// Last observed [`CandleSeries::version`] at paint entry. Used by
    /// [`paint_buckets`](Self::paint_buckets) to short-circuit recompute
    /// of session-band / separator buffers when the series has not
    /// advanced — tracking R1's "avoid rebuilding sessions-buffer if
    /// version hasn't changed" follow-on.
    version_at_paint: u64,
}

impl SessionChart {
    /// Construct the widget. Calendar decides the axis shape —
    /// continuous for crypto, compressed for XNYS. Caller-supplied
    /// time window and viewport width configure the initial axis
    /// range.
    ///
    /// Returns [`SessionChartError::Axis`] on degenerate inputs
    /// (zero/negative viewport width, inverted time range, etc.) so
    /// the host can close the pending window rather than crash on a
    /// silent panic. App-harden M5.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        driver: Arc<SessionChartDriver>,
        calendar: &'static dyn ExchangeCalendar,
        period: BarPeriod,
        price_range: PriceRange,
        viewport: Viewport,
        palette: ThemePalette,
        time_window: (Timestamp, Timestamp),
    ) -> Result<Self, SessionChartError> {
        let series = driver.series();
        let axis = AxisBox::try_for_calendar(calendar, time_window, viewport.width_px)?;
        Ok(Self {
            series,
            driver,
            axis,
            price_range,
            viewport,
            palette,
            calendar,
            period,
            interaction: InteractionState::new(),
            eh_policy: EhPolicy::default(),
            time_window,
            primitives: ScenePrimitives::default(),
            // Bootstrap sentinel — mismatched against any real
            // `series.version()` so the first paint always runs the
            // full band/separator compute path.
            version_at_paint: u64::MAX,
        })
    }

    // ── Mutators ────────────────────────────────────────────────────

    /// Replace the viewport (window resize). Rebuilds the axis at the
    /// new width so pixel-space queries stay correct.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        self.rebuild_axis();
    }

    /// Replace the price range (auto-scale / zoom Y).
    pub fn set_price_range(&mut self, pr: PriceRange) {
        self.price_range = pr;
    }

    /// Replace the session-band / separator warm-up window AND the
    /// axis range. Called by the host on pan/zoom.
    pub fn set_time_window(&mut self, window: (Timestamp, Timestamp)) {
        self.time_window = window;
        self.rebuild_axis();
    }

    /// Replace the bar period. The host is responsible for re-
    /// subscribing the driver through a fresh
    /// [`crate::session_chart::SessionChartDriver::spawn`] before
    /// calling this mutator — the widget just reflects the new state.
    pub fn set_period(&mut self, period: BarPeriod) {
        self.period = period;
    }

    /// Replace the [`EhPolicy`]. Re-subscription through the stream
    /// filter is the host's concern (see
    /// `session_chart/mod.rs` module docs); the widget just reflects
    /// the new state.
    pub fn set_eh_policy(&mut self, policy: EhPolicy) {
        self.eh_policy = policy;
    }

    /// Cycle through the three [`EhPolicy`] variants. Returns the new
    /// policy so the host can use the return value for re-subscribe.
    pub fn cycle_eh_policy(&mut self) -> EhPolicy {
        let next = self.eh_policy.next();
        self.eh_policy = next;
        next
    }

    /// Update the crosshair position. Called from `CursorMoved` events.
    pub fn set_crosshair(&mut self, px: (f32, f32)) {
        self.interaction.crosshair_px = Some(px);
    }

    /// Clear the crosshair (pointer left the widget).
    pub fn clear_crosshair(&mut self) {
        self.interaction.crosshair_px = None;
    }

    /// Replace the axis directly. Rare — the host typically hands pan/
    /// zoom through [`set_time_window`] so the axis kind stays
    /// calendar-consistent. Provided for tests that want fine-grained
    /// control.
    pub fn set_axis(&mut self, axis: ContinuousAxis) {
        self.axis = AxisBox::Continuous(axis);
    }

    // ── Slice 2b: pan / zoom / keyboard helpers ─────────────────────

    /// Pan the visible time window horizontally by `dx_px`. Positive
    /// shifts right, negative shifts left. Rebuilds the axis at the
    /// new window so pixel-space queries stay correct.
    pub fn pan_x(&mut self, dx_px: f32) {
        let new = midas_scene::interaction::pan_time_window(
            self.time_window,
            dx_px,
            self.viewport.width_px,
        );
        tracing::debug!(
            target: "midas_app::session_chart::pan_x",
            dx_px,
            "pan x",
        );
        self.set_time_window(new);
    }

    /// Zoom the x-axis about a pixel anchor. `factor < 1.0` zooms in
    /// (narrows the visible span), `> 1.0` zooms out. Uses the
    /// period's wall-clock duration as the min-10-candles floor.
    pub fn zoom_x_at(&mut self, anchor_x_px: f32, factor: f32) {
        let candle_width_ns = bar_period_ns(self.period);
        let new = midas_scene::interaction::zoom_time_window_at(
            self.time_window,
            anchor_x_px,
            self.viewport.width_px,
            factor,
            candle_width_ns,
        );
        tracing::debug!(
            target: "midas_app::session_chart::zoom_x_at",
            anchor_x_px,
            factor,
            "zoom x",
        );
        self.set_time_window(new);
    }

    /// Zoom the y-axis (price) about a pixel anchor. No-op on
    /// degenerate inputs (factor ≤ 0, non-finite anchor).
    pub fn zoom_y_at(&mut self, anchor_y_px: f32, factor: f32) {
        if let Some(new_range) = midas_scene::interaction::zoom_price_range_at(
            self.price_range,
            anchor_y_px,
            self.viewport.height_px,
            factor,
        ) {
            tracing::debug!(
                target: "midas_app::session_chart::zoom_y_at",
                anchor_y_px,
                factor,
                "zoom y",
            );
            self.price_range = new_range;
        }
    }

    /// Jump the x-window to the FIRST bar in the series. No-op on an
    /// empty series. Keeps the current span.
    pub fn jump_home(&mut self) {
        let guard = self.series.read();
        let Some(first) = guard.at(0) else { return };
        let first_ts = first.ts_open();
        let span = self.time_window.1 - self.time_window.0;
        drop(guard);
        tracing::debug!(
            target: "midas_app::session_chart::jump_home",
            first_ts = %first_ts,
            "jump to first bar",
        );
        self.set_time_window((first_ts, first_ts + span));
    }

    /// Jump the x-window so the LAST bar sits at the right edge.
    /// No-op on an empty series. Keeps the current span.
    pub fn jump_end(&mut self) {
        let guard = self.series.read();
        let n = guard.len();
        if n == 0 {
            return;
        }
        let last = guard.at(n - 1).expect("bounds checked");
        let last_ts = last.ts_open();
        let span = self.time_window.1 - self.time_window.0;
        drop(guard);
        tracing::debug!(
            target: "midas_app::session_chart::jump_end",
            last_ts = %last_ts,
            "jump to last bar",
        );
        self.set_time_window((last_ts - span, last_ts));
    }

    /// Auto-scale the price range to fit the visible candles.
    /// Returns `true` iff the range changed. Used by the "first-ever
    /// data arrived, no saved viewport" path and by an explicit
    /// user-triggered auto-scale shortcut.
    pub fn auto_scale_price_to_visible(&mut self) -> bool {
        let guard = self.series.read();
        let n = guard.len();
        if n == 0 {
            return false;
        }
        // Visible range: the first → last bar (could be narrowed to a
        // binary-search of the current time_window — left as a
        // follow-up slice when a windowed view is needed).
        let visible = 0..n;
        let Some(new_range) = midas_scene::interaction::auto_scale_price(&guard, visible) else {
            return false;
        };
        drop(guard);
        if new_range != self.price_range {
            tracing::debug!(
                target: "midas_app::session_chart::auto_scale_price",
                low = new_range.low(),
                high = new_range.high(),
                "auto-scale price",
            );
            self.price_range = new_range;
            true
        } else {
            false
        }
    }

    /// Call once per frame — if the series has just gone from empty
    /// to non-empty AND no external caller has pinned a price range,
    /// auto-scale to fit. Returns `true` if auto-scale ran.
    ///
    /// "First-ever transition" is signalled by `version_at_paint ==
    /// u64::MAX` (the bootstrap sentinel) matched against a
    /// non-zero live series version.
    pub fn auto_scale_on_first_data(&mut self) -> bool {
        let v = { self.series.read().version() };
        let is_first_data = self.version_at_paint == u64::MAX && v > 0;
        if !is_first_data {
            return false;
        }
        self.auto_scale_price_to_visible()
    }
}

/// Approximate wall-clock nanoseconds per bar for the given period.
/// Used by zoom clamping to enforce the min-10-candles floor on an
/// unknown calendar (compressed gaps are ignored — the clamp is
/// intentionally loose).
fn bar_period_ns(period: BarPeriod) -> i64 {
    use chrono::Duration;
    match period {
        p if p == BarPeriod::m1() => Duration::minutes(1).num_nanoseconds().unwrap_or(0),
        p if p == BarPeriod::m5() => Duration::minutes(5).num_nanoseconds().unwrap_or(0),
        p if p == BarPeriod::d1_rth() => Duration::days(1).num_nanoseconds().unwrap_or(0),
        p if p == BarPeriod::w1() => Duration::days(7).num_nanoseconds().unwrap_or(0),
        // Fallback for any BarPeriod we didn't special-case — a
        // minute's worth of ns is close enough for a zoom floor.
        _ => Duration::minutes(1).num_nanoseconds().unwrap_or(0),
    }
}

impl SessionChart {
    /// Replace the axis with a compressed one directly.
    pub fn set_compressed_axis(&mut self, axis: CompressedAxis) {
        self.axis = AxisBox::Compressed(Box::new(axis));
    }

    /// Rebuild the time axis in-place. If the new inputs are
    /// degenerate (e.g. a Viewport with zero width during a resize
    /// animation), the existing axis is preserved and a warning is
    /// logged — callers don't need the axis to vanish mid-interaction.
    fn rebuild_axis(&mut self) {
        match AxisBox::try_for_calendar(self.calendar, self.time_window, self.viewport.width_px) {
            Ok(ax) => self.axis = ax,
            Err(e) => {
                tracing::warn!(
                    width = self.viewport.width_px,
                    from = %self.time_window.0,
                    to = %self.time_window.1,
                    error = %e,
                    "session_chart: axis rebuild rejected; keeping previous axis",
                );
            }
        }
    }

    // ── Observers ───────────────────────────────────────────────────

    /// Current axis kind (as picked by the calendar).
    pub fn axis_kind(&self) -> AxisKind {
        self.axis.kind()
    }

    /// Current EhPolicy.
    pub fn eh_policy(&self) -> EhPolicy {
        self.eh_policy
    }

    /// Current period.
    pub fn period(&self) -> BarPeriod {
        self.period
    }

    /// Current calendar.
    pub fn calendar(&self) -> &'static dyn ExchangeCalendar {
        self.calendar
    }

    /// Borrow the interaction state.
    pub fn interaction(&self) -> &InteractionState {
        &self.interaction
    }

    /// Current viewport.
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Current price range.
    pub fn price_range(&self) -> PriceRange {
        self.price_range
    }

    /// Reference to the shared candle series. Tests use this to read
    /// len / version after driving the pipeline.
    pub fn series(&self) -> SharedCandleSeries {
        Arc::clone(&self.series)
    }

    /// End-to-end: snapshot the current series version, build a scene
    /// (sharing the same `Arc<RwLock<CandleSeries>>` with the driver —
    /// no deep-copy per frame, arch-audit F1 / R1), paint primitives,
    /// translate into [`RenderBuckets`]. This is what the eventual
    /// shader `Program::draw` will call to hand GPU-layout data to
    /// `wgpu`.
    ///
    /// The session-band / separator buffers and the holiday-marker
    /// year range are recomputed only when the series version has
    /// advanced since the last paint — the common "hover redraw on
    /// unchanged series" path stays allocation-free.
    ///
    /// # Panics
    ///
    /// Panics if `build_scene` errors. With the canonical construction
    /// path all three required builder inputs (axis/price_range/
    /// viewport) are set, so an error here indicates a programming
    /// mistake.
    pub fn paint_buckets(&mut self) -> RenderBuckets {
        // Version check — skip band/separator recompute when unchanged.
        // The read-guard is short-lived and released before `build_scene`.
        let current_version = { self.series.read().version() };
        let series_changed = current_version != self.version_at_paint;
        self.version_at_paint = current_version;
        // Select the matching axis variant and dispatch to
        // `build_scene` with the right monomorphisation.
        let is_xnys = self.axis.kind() == AxisKind::Compressed;
        let layers = self.layers_for_policy(is_xnys);
        let scene = match &self.axis {
            AxisBox::Continuous(axis) => build_scene(SceneConfig {
                series: Arc::clone(&self.series),
                axis: axis.clone(),
                price_range: self.price_range,
                viewport: self.viewport,
                palette: self.palette,
                calendar: self.calendar,
                interaction: &self.interaction,
                layers,
                time_window: self.time_window,
                series_changed,
            }),
            AxisBox::Compressed(axis) => build_scene(SceneConfig {
                series: Arc::clone(&self.series),
                axis: (**axis).clone(),
                price_range: self.price_range,
                viewport: self.viewport,
                palette: self.palette,
                calendar: self.calendar,
                interaction: &self.interaction,
                layers,
                time_window: self.time_window,
                series_changed,
            }),
        }
        .expect("build_scene with canonical inputs");
        scene.paint(&mut self.primitives);
        translate(&self.primitives)
    }

    /// Convenience wrapper around [`SceneLayers::from_eh_policy`]
    /// parameterised by whether this chart sits on XNYS. Exposed for
    /// unit tests that want to assert the layer config separately
    /// from the paint step.
    pub fn layers_for_policy(&self, is_xnys: bool) -> SceneLayers {
        SceneLayers::from_eh_policy(self.eh_policy, is_xnys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use midas_bars::{BarPeriod, Candle, Completeness, Ohlcv, Symbol};
    use midas_calendar::{crypto_spot, xnys};
    use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
    use tokio::sync::mpsc;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_crypto(ts: Timestamp, price: f64) -> Candle {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(price, price + 10.0, price - 10.0, price + 5.0, 1, 1, None).unwrap();
        Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap()
    }

    struct MockStream {
        rx: mpsc::Receiver<Candle>,
        meta: BarStreamMeta,
    }

    impl MockStream {
        fn crypto() -> (mpsc::Sender<Candle>, Self) {
            let cal = crypto_spot();
            let sym = Symbol::new("BTC-USD", cal.id());
            let (tx, rx) = mpsc::channel(32);
            let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
            (tx, Self { rx, meta })
        }

        fn xnys() -> (mpsc::Sender<Candle>, Self) {
            let cal = xnys();
            let sym = Symbol::new("AAPL", cal.id());
            let (tx, rx) = mpsc::channel(32);
            let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
            (tx, Self { rx, meta })
        }
    }

    #[async_trait]
    impl BarStream for MockStream {
        fn meta(&self) -> &BarStreamMeta {
            &self.meta
        }
        async fn next(&mut self) -> Option<Candle> {
            self.rx.recv().await
        }
        async fn snapshot(&mut self, _range: TimeRange) -> Result<Vec<Candle>, StreamError> {
            Err(StreamError::NotSeekable)
        }
    }

    fn fresh_crypto_series() -> Arc<RwLock<CandleSeries>> {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    fn fresh_xnys_series() -> Arc<RwLock<CandleSeries>> {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    fn crypto_widget(driver: Arc<SessionChartDriver>) -> SessionChart {
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
        .expect("canonical crypto widget inputs must succeed")
    }

    fn xnys_widget(driver: Arc<SessionChartDriver>) -> SessionChart {
        let cal = xnys();
        // Two weekdays so CompressedAxis has sessions to work with.
        let start = utc(2024, 1, 17, 0, 0);
        let end = utc(2024, 1, 19, 0, 0);
        let pr = PriceRange::new(180.0, 200.0).unwrap();
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
        .expect("canonical xnys widget inputs must succeed")
    }

    // ── Part A: axis kind picked from calendar ──────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_widget_has_continuous_axis() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let w = crypto_widget(driver);
        assert_eq!(w.axis_kind(), AxisKind::Continuous);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_widget_has_compressed_axis() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let w = xnys_widget(driver);
        assert_eq!(w.axis_kind(), AxisKind::Compressed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn axis_kind_for_calendar_matches_policy() {
        // Static helper — no widget needed.
        assert_eq!(AxisKind::for_calendar(crypto_spot()), AxisKind::Continuous);
        assert_eq!(AxisKind::for_calendar(xnys()), AxisKind::Compressed);
    }

    // ── Part D: EhPolicy cycling + layer derivation ─────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn default_eh_policy_is_show_all() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let w = crypto_widget(driver);
        assert_eq!(w.eh_policy(), EhPolicy::ShowAll);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cycle_eh_policy_round_trips_after_three_calls() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        assert_eq!(w.eh_policy(), EhPolicy::ShowAll);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::HideExtended);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::ShowBarsOnly);
        assert_eq!(w.cycle_eh_policy(), EhPolicy::ShowAll);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_show_all_layers_include_holidays_bands_and_separators() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let w = xnys_widget(driver);
        let layers = w.layers_for_policy(true);
        assert!(layers.session_bands);
        assert!(layers.session_separators);
        assert!(layers.holidays);
        assert!(layers.candles);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn show_bars_only_drops_bands_and_separators() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut w = xnys_widget(driver);
        w.set_eh_policy(EhPolicy::ShowBarsOnly);
        let layers = w.layers_for_policy(true);
        assert!(!layers.session_bands);
        assert!(!layers.session_separators);
        // Holidays survive — they are per-calendar not per-EhPolicy.
        assert!(layers.holidays);
        assert!(layers.candles);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_never_enables_holiday_layer() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        for _ in 0..3 {
            let layers = w.layers_for_policy(false);
            assert!(!layers.holidays);
            w.cycle_eh_policy();
        }
    }

    // ── Part B: period picker reflection ────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_period_updates_widget_field() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut w = xnys_widget(driver);
        assert_eq!(w.period(), BarPeriod::m1());
        w.set_period(BarPeriod::d1_rth());
        assert_eq!(w.period(), BarPeriod::d1_rth());
        w.set_period(BarPeriod::w1());
        assert_eq!(w.period(), BarPeriod::w1());
    }

    // ── Paint smoke tests ───────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_paint_buckets_on_empty_series_yields_nonzero_grid() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut widget = crypto_widget(driver);
        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 0);
        assert!(!b.lines.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crypto_paint_buckets_after_candles_emits_candle_bucket() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));

        let start = utc(2024, 3, 1, 0, 0);
        for i in 0..5 {
            tx.send(mk_crypto(start + chrono::Duration::minutes(i), 50_000.0))
                .await
                .unwrap();
        }
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 5 {
            if rx.changed().await.is_err() {
                break;
            }
        }

        let mut widget = crypto_widget(Arc::clone(&driver));
        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_paint_buckets_on_empty_series_yields_nonzero_grid() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut widget = xnys_widget(driver);
        let b = widget.paint_buckets();
        assert_eq!(b.candles.len(), 0);
        // Grid + bands should emit.
        assert!(!b.lines.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crosshair_toggle_round_trips_through_paint() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut widget = crypto_widget(driver);

        let before = widget.paint_buckets();
        let before_lines = before.lines.len();

        widget.set_crosshair((400.0, 200.0));
        let after = widget.paint_buckets();
        assert!(after.lines.len() >= before_lines + 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_viewport_rebuilds_axis_at_new_width() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut widget = crypto_widget(driver);
        widget.set_viewport(Viewport::new(1600.0, 900.0));
        assert!((widget.viewport().width_px - 1600.0).abs() < 1e-3);
        assert!((widget.axis.width_px() - 1600.0).abs() < 1e-3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn xnys_set_viewport_preserves_compressed_axis_kind() {
        let (tx, stream) = MockStream::xnys();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_xnys_series(), stream));
        let mut widget = xnys_widget(driver);
        assert_eq!(widget.axis_kind(), AxisKind::Compressed);
        widget.set_viewport(Viewport::new(2400.0, 1200.0));
        assert_eq!(widget.axis_kind(), AxisKind::Compressed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn eh_policy_short_label_exposed() {
        assert_eq!(EhPolicy::ShowAll.short_label(), "EH");
        assert_eq!(EhPolicy::HideExtended.short_label(), "RTH");
        assert_eq!(EhPolicy::ShowBarsOnly.short_label(), "EH·bars");
    }

    /// Regression: app-harden M5. `SessionChart::new` now returns a
    /// `Result` on degenerate inputs (inverted time range, zero width)
    /// instead of panicking via `.expect`. Host code can close the
    /// half-opened window gracefully.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn new_returns_err_on_inverted_time_range() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let cal = crypto_spot();
        let start = utc(2024, 3, 2, 0, 0);
        let end = utc(2024, 3, 1, 0, 0); // end < start → invalid.
        let pr = PriceRange::new(100.0, 200.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let result = SessionChart::new(
            driver,
            cal,
            BarPeriod::m1(),
            pr,
            vp,
            ThemePalette::dark_default(),
            (start, end),
        );
        assert!(
            matches!(result, Err(SessionChartError::Axis(_))),
            "inverted time range must surface as Err, not panic"
        );
    }

    /// Regression: bug-hunt H5. The pre-R1 `shared_series_snapshot`
    /// deep-copied every row and accidentally hard-coded
    /// `trade_count=0, wap=None`, silently losing both values. The
    /// post-R1 widget shares the same `Arc<RwLock<CandleSeries>>` as
    /// the driver — no deep copy — so `trade_count` and `wap` arrive
    /// intact. This test pushes a candle with explicit values and
    /// reads them back from the shared series behind a read-guard.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shared_series_preserves_trade_count_and_wap() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));

        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let ts = utc(2024, 3, 1, 0, 0);
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(
            50_000.0,
            50_100.0,
            49_900.0,
            50_050.0,
            12_345,
            77, // trade_count
            Some(50_025.5),
        )
        .unwrap();
        let candle = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        tx.send(candle).await.unwrap();
        drop(tx);

        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }

        let widget = crypto_widget(Arc::clone(&driver));
        let shared = widget.series();
        let guard = shared.read();
        assert_eq!(guard.len(), 1);
        let row = guard.at(0).unwrap();
        assert_eq!(row.trade_count(), 77);
        assert_eq!(row.wap(), Some(50_025.5));
    }

    /// Smoke benchmark for R1: painting a 10k-row series 100 times must
    /// complete in well under an eye-balling budget. The old
    /// `shared_series_snapshot` path deep-copied every row through the
    /// calendar on every paint — an easy O(n*paint_count) pathological
    /// case. The post-R1 widget shares the driver's
    /// `Arc<RwLock<CandleSeries>>` directly, so paint cost is O(n) per
    /// call, not O(n*k) for k-paints.
    ///
    /// The bound is intentionally loose (5 seconds for 100 paints over
    /// 10k rows) so CI flakiness doesn't derail green builds. The
    /// purpose is to catch regressions that reintroduce a per-frame
    /// deep copy — those push run times into the tens of seconds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paint_buckets_10k_rows_100_paints_no_quadratic_blowup() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));

        // Pre-populate the series with 10_000 rows by pushing directly —
        // bypasses the pump task so the driver isn't racing our paint
        // loop below.
        {
            let cal = crypto_spot();
            let sym = Symbol::new("BTC-USD", cal.id());
            let start = utc(2024, 3, 1, 0, 0);
            let mut guard = series.write();
            for i in 0..10_000i64 {
                let ts = start + chrono::Duration::minutes(i);
                let session = cal.classify(ts);
                let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
                let price = 50_000.0 + (i as f64) * 0.01;
                let ohlcv = Ohlcv::new(price, price + 0.5, price - 0.5, price + 0.25, 100, 1, None)
                    .unwrap();
                let candle = Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap();
                guard.push(candle);
            }
        }
        drop(tx);

        let mut widget = crypto_widget(Arc::clone(&driver));
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let _ = widget.paint_buckets();
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "100 paints over 10k rows took {elapsed:?} — regression suspected (R1)"
        );
    }

    // ── Slice 2b: pan / zoom / keyboard / auto-scale ────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pan_x_shifts_time_window_forward() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let (s0, e0) = w.time_window;
        w.pan_x(100.0);
        let (s1, e1) = w.time_window;
        assert!(s1 > s0);
        assert!(e1 > e0);
        assert_eq!(e1 - s1, e0 - s0, "span preserved");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_x_in_narrows_span_and_preserves_anchor() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let (s0, e0) = w.time_window;
        let span0 = e0 - s0;
        let anchor_px = w.viewport().width_px * 0.5;
        w.zoom_x_at(anchor_px, 0.5);
        let (s1, e1) = w.time_window;
        let span1 = e1 - s1;
        assert!(span1 < span0, "span narrowed on zoom-in");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_y_in_narrows_price_span() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let before = w.price_range();
        w.zoom_y_at(w.viewport().height_px * 0.5, 0.5);
        let after = w.price_range();
        assert!(after.span() < before.span());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jump_home_positions_first_bar_at_left() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));
        // Push one candle so the series is non-empty.
        let ts = utc(2024, 3, 5, 0, 0);
        tx.send(mk_crypto(ts, 50_000.0)).await.unwrap();
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let mut w = crypto_widget(Arc::clone(&driver));
        w.jump_home();
        assert_eq!(w.time_window.0, ts);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jump_home_on_empty_series_is_noop() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let (s0, e0) = w.time_window;
        w.jump_home();
        assert_eq!(w.time_window.0, s0);
        assert_eq!(w.time_window.1, e0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn jump_end_positions_last_bar_at_right() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));
        let ts = utc(2024, 3, 5, 0, 0);
        tx.send(mk_crypto(ts, 50_000.0)).await.unwrap();
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let mut w = crypto_widget(Arc::clone(&driver));
        w.jump_end();
        assert_eq!(w.time_window.1, ts);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_scale_on_first_data_sets_price_range() {
        let series = fresh_crypto_series();
        let (tx, stream) = MockStream::crypto();
        let driver = Arc::new(SessionChartDriver::spawn(Arc::clone(&series), stream));
        // Push one candle with known O/H/L/C.
        let ts = utc(2024, 3, 5, 0, 0);
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(50_000.0, 50_100.0, 49_900.0, 50_050.0, 1, 1, None).unwrap();
        let candle = Candle::new(
            sym,
            cal,
            BarPeriod::m1(),
            session,
            window,
            ohlcv,
            Completeness::Completed,
        )
        .unwrap();
        tx.send(candle).await.unwrap();
        drop(tx);
        let mut rx = driver.version_receiver();
        while *rx.borrow_and_update() < 1 {
            if rx.changed().await.is_err() {
                break;
            }
        }
        let mut w = crypto_widget(Arc::clone(&driver));
        let fired = w.auto_scale_on_first_data();
        assert!(fired, "auto-scale must run on first-data transition");
        let r = w.price_range();
        // Fits high=50_100, low=49_900 with 5% pad.
        assert!(r.low() < 49_900.0);
        assert!(r.high() > 50_100.0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn auto_scale_on_first_data_no_op_when_empty() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        assert!(!w.auto_scale_on_first_data());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_x_min_floor_at_10_candles() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        // Extreme zoom-in factor.
        w.zoom_x_at(500.0, 0.00001);
        let span = w.time_window.1 - w.time_window.0;
        // 10 × 1-minute min floor = 10 minutes.
        assert!(span >= chrono::Duration::minutes(10));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zoom_y_no_op_on_nan_anchor() {
        let (tx, stream) = MockStream::crypto();
        drop(tx);
        let driver = Arc::new(SessionChartDriver::spawn(fresh_crypto_series(), stream));
        let mut w = crypto_widget(driver);
        let before = w.price_range();
        w.zoom_y_at(f32::NAN, 0.5);
        assert_eq!(before, w.price_range());
    }
}
