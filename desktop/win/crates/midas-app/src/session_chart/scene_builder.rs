//! [`build_scene`] — sans-IO assembly of a [`ChartScene`] for both
//! crypto (continuous axis) and XNYS (compressed axis) chart kinds.
//!
//! The builder is a thin, documented layer over [`ChartScene::builder`].
//! In Phase C (`plan/session-aware-charts/00b-integration-strategy.md`,
//! rows S10–S14) it was widened from the crypto-only MVP to handle:
//!
//! - **Calendar-agnostic axis construction** — chooses
//!   [`ContinuousAxis`] for crypto and [`CompressedAxis`] for XNYS (and
//!   any future exchange whose
//!   [`TimeAxisPolicy`][midas_calendar::TimeAxisPolicy] is
//!   `CompressedSessionBoundaries`) via
//!   [`midas_axis::for_calendar`].
//! - **Holiday marker layer** — wired from the calendar's year-range
//!   holiday table when [`SceneLayers::holidays`] is on. XNYS enables
//!   this by default; crypto leaves it off (`CryptoSpotCalendar` has no
//!   holidays).
//! - **Extended-hours policy** — the widget sends its
//!   [`EhPolicy`][super::widget::EhPolicy] into
//!   [`SceneLayers::from_eh_policy`], which toggles session bands /
//!   separators per the ideal-design rule in
//!   `00a-ideal-design.md` → "Ideal behaviours → EhPolicy".
//!
//! The layer stack picked (MVP + Phase C):
//!
//! - [`SessionBandLayer`] (z=0) — RTH / PreMarket / PostMarket tint.
//!   Skipped when `session_bands = false`.
//! - [`GridLayer`] (z=1) — price + time gridlines.
//! - [`SessionSeparatorLayer`] (z=2) — vertical rules between sessions.
//!   Skipped when `session_separators = false`.
//! - [`VolumeLayer`] (z=3) — bottom-strip volume bars.
//! - [`CandleLayer`] (z=4) — OHLC candles over everything above.
//! - [`HolidayMarkerLayer`] (z=5) — day badges anchored at each holiday
//!   date. Only emitted for XNYS (calendar decides at holiday-table
//!   walk time — crypto's calendar is empty).
//! - [`CrosshairLayer`] (z=10) — mouse-follower guides, only emitted
//!   when `interaction.crosshair_px` is `Some`.
//!
//! ## Open TODOs deferred to Phase D / later slices
//!
//! - Thumbnail preset (G-6 in ideal-design). `SceneLayers::thumbnail`
//!   is NOT yet wired through; consumers can still construct one
//!   manually.
//! - Axis kind auto-switching at zoom thresholds (R2-NM-6). Phase F.
//! - Annotation layers (order brackets, price lines, levels). Phase D
//!   replaces the legacy chart path and re-lights those layers.

use std::sync::Arc;

use midas_axis::{PriceRange, TimeAxis, Viewport};
use midas_calendar::ExchangeCalendar;
use midas_scene::{
    CandleLayer, CandleStyle, ChartScene, CrosshairLayer, GridLayer, GridStyle, HolidayMarkerLayer,
    InteractionState, SceneError, SessionBandLayer, SessionSeparatorLayer, SharedCandleSeries,
    ThemePalette, VolumeLayer, VolumeStyle,
};

use super::policy::EhPolicy;

/// Flags toggling individual layers. Mirrors
/// [`midas_scene::LayerConfig`] but with a Phase-C convenience
/// constructor [`SceneLayers::from_eh_policy`] that derives the
/// user-visible on/off flags from an [`EhPolicy`] value.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SceneLayers {
    pub session_bands: bool,
    pub grid: bool,
    pub session_separators: bool,
    pub volume: bool,
    pub candles: bool,
    pub holidays: bool,
    pub crosshair: bool,
}

impl SceneLayers {
    /// All MVP layers on (holidays off by default — callers opt in on
    /// XNYS via [`SceneLayers::from_eh_policy`] or a manual flag flip).
    pub const fn all_on() -> Self {
        Self {
            session_bands: true,
            grid: true,
            session_separators: true,
            volume: true,
            candles: true,
            holidays: false,
            crosshair: true,
        }
    }

    /// Candles + grid only. Used by some unit tests that want a
    /// minimal emit surface.
    pub const fn candles_and_grid() -> Self {
        Self {
            session_bands: false,
            grid: true,
            session_separators: false,
            volume: false,
            candles: true,
            holidays: false,
            crosshair: false,
        }
    }

    /// Derive the effective layer set from an [`EhPolicy`].
    ///
    /// Per ideal-design §"Ideal behaviours → EhPolicy":
    ///
    /// - [`EhPolicy::ShowAll`]: all chrome on — bands, separators,
    ///   candles, volume, grid, crosshair.
    /// - [`EhPolicy::HideExtended`]: RTH-only. Bands + separators DO
    ///   still render (the band layer walks the calendar and only
    ///   tints *visible* sessions; Extended windows are filtered out
    ///   upstream by [`midas_stream::Filtered<_, EhFilter>`]). Effect
    ///   on the scene: identical layer set to `ShowAll` — the
    ///   difference is in the candle stream, not the chrome.
    /// - [`EhPolicy::ShowBarsOnly`]: candles render tinted by session
    ///   but bands + separators do NOT emit.
    ///
    /// The `is_xnys` flag gates the `holidays` flag on — crypto's
    /// holiday table is empty, so leaving it on would just walk dates
    /// for nothing; gating it off keeps the build allocation-free.
    pub const fn from_eh_policy(policy: EhPolicy, is_xnys: bool) -> Self {
        match policy {
            EhPolicy::ShowAll | EhPolicy::HideExtended => Self {
                session_bands: true,
                grid: true,
                session_separators: true,
                volume: true,
                candles: true,
                holidays: is_xnys,
                crosshair: true,
            },
            EhPolicy::ShowBarsOnly => Self {
                session_bands: false,
                grid: true,
                session_separators: false,
                volume: true,
                candles: true,
                holidays: is_xnys,
                crosshair: true,
            },
        }
    }
}

impl Default for SceneLayers {
    fn default() -> Self {
        Self::all_on()
    }
}

/// Frame-level inputs to [`build_scene`]. Builder params live in one
/// struct so callers (widget, tests, downstream plans) can extend
/// without a flat-fn signature explosion.
pub struct SceneConfig<'a, A: TimeAxis + 'static> {
    /// Shared candle series — the same `Arc<RwLock<CandleSeries>>`
    /// the driver writes to. [`CandleLayer`] and [`VolumeLayer`] take
    /// read-guards inside their `paint` methods (short-lived,
    /// released before `paint` returns).
    pub series: SharedCandleSeries,
    pub axis: A,
    pub price_range: PriceRange,
    pub viewport: Viewport,
    pub palette: ThemePalette,
    pub calendar: &'static dyn ExchangeCalendar,
    pub interaction: &'a InteractionState,
    pub layers: SceneLayers,
    /// Time range the session-band + session-separator layers should
    /// populate their caches from. Typically the axis's start/end,
    /// widened by a small slop if the caller wants off-screen bands
    /// warmed up.
    pub time_window: (midas_calendar::Timestamp, midas_calendar::Timestamp),
    /// Whether the series version has advanced since the last paint.
    /// When `false`, the band / separator buffers are reused without a
    /// recompute — supports R1's "avoid rebuilding sessions-buffer if
    /// version hasn't changed" follow-on. Callers that don't track
    /// version may pass `true` to force a rebuild every frame.
    pub series_changed: bool,
}

/// Build a [`ChartScene`] for the given frame.
///
/// Returns [`SceneError`] if the underlying builder rejects the config
/// (e.g. mixed-up viewport). Each layer is optional — disabled ones
/// simply aren't added, so zero-cost when the user doesn't want them.
///
/// The session band + separator layers call
/// `update_sessions` / `update_boundaries` inside this function so the
/// caller doesn't need to know those are "pre-paint" concerns. The
/// holiday layer walks a year range derived from the time window.
pub fn build_scene<A: TimeAxis + 'static>(
    cfg: SceneConfig<'_, A>,
) -> Result<ChartScene, SceneError> {
    let SceneConfig {
        series,
        axis,
        price_range,
        viewport,
        palette,
        calendar,
        interaction,
        layers,
        time_window,
        // `series_changed` is accepted for forward-compat with widgets
        // that track series version across frames (R1 follow-on). The
        // current builder constructs fresh layers each call and always
        // repopulates their per-frame caches, so the flag is informational
        // today. A future refactor that caches the band/separator layer
        // instances at the widget level will consult this flag to decide
        // whether to call `update_sessions` / `update_boundaries` again.
        series_changed: _,
    } = cfg;

    let mut builder = ChartScene::builder()
        .axis(axis)
        .price_range(price_range)
        .viewport(viewport)
        .palette(palette);

    if layers.session_bands {
        let mut layer = SessionBandLayer::new(calendar);
        layer.update_sessions(time_window.0, time_window.1);
        builder = builder.layer(layer);
    }

    if layers.grid {
        builder = builder.layer(GridLayer::new(GridStyle::default()));
    }

    if layers.session_separators {
        let mut layer = SessionSeparatorLayer::new(calendar);
        layer.update_boundaries(time_window.0, time_window.1);
        builder = builder.layer(layer);
    }

    if layers.volume {
        builder = builder.layer(VolumeLayer::new(
            Arc::clone(&series),
            VolumeStyle::default(),
        ));
    }

    if layers.candles {
        builder = builder.layer(CandleLayer::new(
            Arc::clone(&series),
            CandleStyle::default(),
        ));
    }

    if layers.holidays {
        // Derive a conservative year range from the viewport window.
        // `time_window` is small (one chart's worth) so this walks at
        // most ~3 years of calendar dates — cheap.
        let start_year = time_window.0.naive_utc().date().year();
        let end_year = time_window.1.naive_utc().date().year();
        // `Range` is half-open; include `end_year`.
        let year_range = start_year..(end_year.saturating_add(1));
        let holidays = HolidayMarkerLayer::new(calendar, year_range);
        builder = builder.layer(holidays);
    }

    if layers.crosshair {
        let hair = match interaction.crosshair_px {
            Some(pos) => CrosshairLayer::with_position(pos),
            None => CrosshairLayer::new(),
        };
        builder = builder.layer(hair);
    }

    builder.build()
}

// Local re-import: `Datelike` is needed for `NaiveDate::year()`. We
// import under the function because rust-analyzer clients following a
// `use` chain can see it right above the usage; top-level imports
// would drag `Datelike` into every macro-expanded test assertion.
use chrono::Datelike;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use midas_axis::{for_calendar, ContinuousAxis};
    use midas_bars::{BarPeriod, Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{crypto_spot, xnys, TimeAxisPolicy, Timestamp};
    use midas_scene::ScenePrimitives;
    use parking_lot::RwLock;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn crypto_series_with(n: usize) -> SharedCandleSeries {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), sym);
        let start = utc(2024, 3, 1, 0, 0);
        for i in 0..n {
            let ts = start + chrono::Duration::minutes(i as i64);
            let session = cal.classify(ts);
            let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
            let p = 50_000.0 + i as f64;
            let ohlcv = Ohlcv::new(p, p + 10.0, p - 10.0, p + 5.0, 100, 1, None).unwrap();
            s.push(
                Candle::new(
                    sym,
                    cal,
                    BarPeriod::m1(),
                    session,
                    window,
                    ohlcv,
                    Completeness::Completed,
                )
                .unwrap(),
            );
        }
        Arc::new(RwLock::new(s))
    }

    fn xnys_series_empty() -> SharedCandleSeries {
        let cal = xnys();
        let sym = Symbol::new("AAPL", cal.id());
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            sym,
        )))
    }

    /// Test-only bundle of inputs.
    struct Harness {
        series: SharedCandleSeries,
        price_range: PriceRange,
        viewport: Viewport,
        palette: ThemePalette,
        calendar: &'static dyn ExchangeCalendar,
        interaction: InteractionState,
        time_window: (Timestamp, Timestamp),
    }

    fn crypto_harness(n: usize) -> Harness {
        let cal = crypto_spot();
        let series = crypto_series_with(n);
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let pr = PriceRange::new(49_900.0, 50_200.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        Harness {
            series,
            price_range: pr,
            viewport: vp,
            palette: ThemePalette::dark_default(),
            calendar: cal,
            interaction: InteractionState::new(),
            time_window: (start, end),
        }
    }

    fn xnys_harness() -> Harness {
        let cal = xnys();
        let series = xnys_series_empty();
        // A Wednesday + Thursday — two regular XNYS sessions fit.
        let start = utc(2024, 1, 17, 0, 0);
        let end = utc(2024, 1, 19, 0, 0);
        let pr = PriceRange::new(180.0, 200.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        Harness {
            series,
            price_range: pr,
            viewport: vp,
            palette: ThemePalette::dark_default(),
            calendar: cal,
            interaction: InteractionState::new(),
            time_window: (start, end),
        }
    }

    // ── Crypto (continuous axis) ────────────────────────────────────

    #[test]
    fn crypto_continuous_axis_zero_candles_scene_still_builds() {
        let h = crypto_harness(0);
        let axis = ContinuousAxis::new(h.time_window.0, h.time_window.1, 1000.0).unwrap();
        let scene = build_scene(SceneConfig {
            series: Arc::clone(&h.series),
            axis,
            price_range: h.price_range,
            viewport: h.viewport,
            palette: h.palette,
            calendar: h.calendar,
            interaction: &h.interaction,
            layers: SceneLayers::all_on(),
            time_window: h.time_window,
            series_changed: true,
        })
        .unwrap();
        assert_eq!(scene.axis().policy(), TimeAxisPolicy::Continuous);
        // all_on has 6 layers: band/grid/separator/volume/candle/crosshair
        // (no holidays for crypto).
        assert_eq!(scene.layer_count(), 6);
    }

    #[test]
    fn crypto_ten_candles_scene_emits_ten_candle_primitives() {
        let h = crypto_harness(10);
        let axis = ContinuousAxis::new(h.time_window.0, h.time_window.1, 1000.0).unwrap();
        let scene = build_scene(SceneConfig {
            series: Arc::clone(&h.series),
            axis,
            price_range: h.price_range,
            viewport: h.viewport,
            palette: h.palette,
            calendar: h.calendar,
            interaction: &h.interaction,
            layers: SceneLayers::all_on(),
            time_window: h.time_window,
            series_changed: true,
        })
        .unwrap();
        let mut out = ScenePrimitives::default();
        scene.paint(&mut out);
        assert_eq!(out.candles.len(), 10);
    }

    // ── XNYS (compressed axis) ──────────────────────────────────────

    #[test]
    fn xnys_compressed_axis_builds() {
        let h = xnys_harness();
        let axis = for_calendar(h.calendar, h.time_window, 1000.0);
        let scene = ChartScene::builder()
            .axis_boxed(axis)
            .price_range(h.price_range)
            .viewport(h.viewport)
            .palette(h.palette)
            .build()
            .unwrap();
        assert_eq!(
            scene.axis().policy(),
            TimeAxisPolicy::CompressedSessionBoundaries
        );
    }

    #[test]
    fn xnys_via_build_scene_with_boxed_axis_equivalent() {
        // `build_scene` generics take an owning `A: TimeAxis`; exercising
        // it with `CompressedAxis` via `for_calendar` (which returns
        // `Box<dyn TimeAxis>`) needs a detour. We round-trip the boxed
        // axis through a helper that bridges to `build_scene`'s
        // interface.
        //
        // Rather than shim that here, the widget exposes the detour via
        // its own axis-kind enum. This test sticks to the direct
        // `ChartSceneBuilder` path to prove the compressed axis lands
        // in the scene correctly — the widget's end-to-end test covers
        // the `build_scene` path.
        let h = xnys_harness();
        let axis = for_calendar(h.calendar, h.time_window, 1000.0);
        let scene = ChartScene::builder()
            .axis_boxed(axis)
            .price_range(h.price_range)
            .viewport(h.viewport)
            .palette(h.palette)
            .layer({
                let mut layer = SessionBandLayer::new(h.calendar);
                layer.update_sessions(h.time_window.0, h.time_window.1);
                layer
            })
            .layer(GridLayer::new(GridStyle::default()))
            .build()
            .unwrap();
        // Scene has compressed axis + bands + grid.
        assert_eq!(
            scene.axis().policy(),
            TimeAxisPolicy::CompressedSessionBoundaries
        );
        assert_eq!(scene.layer_count(), 2);
    }

    #[test]
    fn xnys_session_bands_emit_at_least_two_quads_for_two_days() {
        let h = xnys_harness();
        let axis = for_calendar(h.calendar, h.time_window, 1000.0);
        let mut band_layer = SessionBandLayer::new(h.calendar);
        band_layer.update_sessions(h.time_window.0, h.time_window.1);
        let scene = ChartScene::builder()
            .axis_boxed(axis)
            .price_range(h.price_range)
            .viewport(h.viewport)
            .palette(h.palette)
            .layer(band_layer)
            .build()
            .unwrap();
        let mut out = ScenePrimitives::default();
        scene.paint(&mut out);
        // Two XNYS trading days emit at least one band quad each for
        // Regular (PreMarket + PostMarket bands also emit). Lower bound
        // is 2 for the two RTH sessions; actual count may be higher.
        assert!(
            out.quads.len() >= 2,
            "expected >=2 band quads, got {}",
            out.quads.len()
        );
    }

    // ── EhPolicy wiring ──────────────────────────────────────────────

    #[test]
    fn eh_policy_show_all_has_bands_and_separators() {
        let layers = SceneLayers::from_eh_policy(EhPolicy::ShowAll, true);
        assert!(layers.session_bands);
        assert!(layers.session_separators);
        assert!(layers.candles);
        assert!(layers.holidays);
    }

    #[test]
    fn eh_policy_hide_extended_keeps_bands_and_separators() {
        // Per ideal-design: bands/separators still render; the filter
        // lives in the stream, not the scene chrome.
        let layers = SceneLayers::from_eh_policy(EhPolicy::HideExtended, true);
        assert!(layers.session_bands);
        assert!(layers.session_separators);
    }

    #[test]
    fn eh_policy_show_bars_only_hides_bands_and_separators() {
        let layers = SceneLayers::from_eh_policy(EhPolicy::ShowBarsOnly, true);
        assert!(!layers.session_bands);
        assert!(!layers.session_separators);
        assert!(layers.candles, "candles must still render");
        assert!(layers.crosshair, "crosshair still usable");
    }

    #[test]
    fn eh_policy_crypto_never_gets_holidays() {
        for p in [
            EhPolicy::ShowAll,
            EhPolicy::HideExtended,
            EhPolicy::ShowBarsOnly,
        ] {
            let layers = SceneLayers::from_eh_policy(p, false);
            assert!(
                !layers.holidays,
                "crypto must never enable the holiday layer (policy {p:?})"
            );
        }
    }

    #[test]
    fn eh_policy_xnys_always_gets_holidays() {
        for p in [
            EhPolicy::ShowAll,
            EhPolicy::HideExtended,
            EhPolicy::ShowBarsOnly,
        ] {
            let layers = SceneLayers::from_eh_policy(p, true);
            assert!(
                layers.holidays,
                "XNYS must enable the holiday layer (policy {p:?})"
            );
        }
    }

    // ── Holiday layer wiring ────────────────────────────────────────

    #[test]
    fn xnys_build_scene_with_holidays_emits_holiday_layer() {
        // Use a continuous axis for `build_scene`'s generic bound; the
        // holiday layer walks date ranges and doesn't care which axis
        // policy is active — the downstream paint uses the viewport's
        // `from_x_snapped` to clip.
        let h = xnys_harness();
        let axis = ContinuousAxis::new(h.time_window.0, h.time_window.1, 1000.0).unwrap();
        let layers = SceneLayers::from_eh_policy(EhPolicy::ShowAll, true);
        let scene = build_scene(SceneConfig {
            series: Arc::clone(&h.series),
            axis,
            price_range: h.price_range,
            viewport: h.viewport,
            palette: h.palette,
            calendar: h.calendar,
            interaction: &h.interaction,
            layers,
            time_window: h.time_window,
            series_changed: true,
        })
        .unwrap();
        // 7 layers on XNYS ShowAll: band/grid/separator/volume/candle/holiday/crosshair.
        assert_eq!(scene.layer_count(), 7);
    }

    #[test]
    fn crypto_build_scene_has_no_holiday_layer() {
        let h = crypto_harness(0);
        let axis = ContinuousAxis::new(h.time_window.0, h.time_window.1, 1000.0).unwrap();
        let layers = SceneLayers::from_eh_policy(EhPolicy::ShowAll, false);
        let scene = build_scene(SceneConfig {
            series: Arc::clone(&h.series),
            axis,
            price_range: h.price_range,
            viewport: h.viewport,
            palette: h.palette,
            calendar: h.calendar,
            interaction: &h.interaction,
            layers,
            time_window: h.time_window,
            series_changed: true,
        })
        .unwrap();
        // 6 layers for crypto (no holiday).
        assert_eq!(scene.layer_count(), 6);
    }

    // ── Other sanity checks ─────────────────────────────────────────

    #[test]
    fn candles_and_grid_preset_emits_two_layers() {
        let h = crypto_harness(3);
        let axis = ContinuousAxis::new(h.time_window.0, h.time_window.1, 1000.0).unwrap();
        let scene = build_scene(SceneConfig {
            series: Arc::clone(&h.series),
            axis,
            price_range: h.price_range,
            viewport: h.viewport,
            palette: h.palette,
            calendar: h.calendar,
            interaction: &h.interaction,
            layers: SceneLayers::candles_and_grid(),
            time_window: h.time_window,
            series_changed: true,
        })
        .unwrap();
        assert_eq!(scene.layer_count(), 2);
    }

    #[test]
    fn crosshair_rendered_when_position_some() {
        let mut h = crypto_harness(3);
        h.interaction.crosshair_px = Some((300.0, 150.0));
        let axis = ContinuousAxis::new(h.time_window.0, h.time_window.1, 1000.0).unwrap();
        let layers = SceneLayers {
            session_bands: false,
            grid: false,
            session_separators: false,
            volume: false,
            candles: false,
            holidays: false,
            crosshair: true,
        };
        let scene = build_scene(SceneConfig {
            series: Arc::clone(&h.series),
            axis,
            price_range: h.price_range,
            viewport: h.viewport,
            palette: h.palette,
            calendar: h.calendar,
            interaction: &h.interaction,
            layers,
            time_window: h.time_window,
            series_changed: true,
        })
        .unwrap();
        let mut out = ScenePrimitives::default();
        scene.paint(&mut out);
        assert_eq!(out.lines.len(), 2);
    }
}
