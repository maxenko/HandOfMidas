//! Per-(symbol, timeframe) chart view state.
//!
//! `ChartViewState` is the single authority for how a chart's camera
//! should be positioned when data loads. It stores the user's zoom
//! levels (X = visible candle count, Y = price zoom factor) and applies
//! them consistently regardless of which code path triggers a data load.
//!
//! Stored in [`ChartViewStore`], a HashMap keyed by `(symbol, timeframe)`.
//!
//! ## Slice 8a — dual-schema migration
//!
//! The store now lives in two parallel maps during the chart-transition
//! migration window:
//!
//! - **v1** — legacy key `(symbol, Timeframe)`. Every write lands here
//!   in addition to v2 so that a reverted binary (which only reads v1)
//!   still finds the user's saved zooms (R6 rollback requirement).
//! - **v2** — session-aware key `(symbol, CalendarIdKey, BarPeriodKey)`
//!   plus the `collapse_gaps` axis-intent flag. Future code reads from
//!   here preferentially.
//!
//! Reader contract: v2 wins when present; otherwise we fall back to v1
//! and opportunistically forward-migrate by inserting the v1 value into
//! v2 on the next write. Slice 9c drops the v1 writes once the new
//! stack owns every call site.
//!
//! Persistence: the `chart_view_store_schema` key on `AppConfig` stamps
//! which schema the binary last wrote. Unknown / missing → assume v1.
//! This is **session-scoped state, not persisted** — the schema key is
//! persisted for rollback coordination, but the views themselves reset
//! across restarts.
//!
//! ## Chart-transition slice 8.5 status
//!
//! The `Camera2D` + `CandleBuffer` imports below are used ONLY by the
//! legacy chart path (`chart_widget.rs` + `app.rs::apply_candle_data`).
//! The session-chart path has its own [`midas_axis::Viewport`] +
//! [`midas_bars::CandleSeries`] equivalents inside
//! [`crate::session_chart`](crate::session_chart). Deleted in slice 9c.

use midas_chart::camera::Camera2D;
use midas_core::{CandleBuffer, Timeframe};

/// Default number of visible candles when no saved zoom exists.
const DEFAULT_VISIBLE_CANDLES: usize = 200;

/// Fraction of data span added as right padding.
/// Places the last candle at ~87.5% of the viewport
/// (middle of the 4th horizontal quadrant).
const RIGHT_PADDING_FRACTION: f64 = 0.14;

/// Default price zoom factor: 1.0 = auto-fit to data range, plus 10%
/// padding on each side. Values < 1.0 = zoomed in (squeezed Y),
/// values > 1.0 = zoomed out (stretched Y).
const DEFAULT_PRICE_ZOOM: f64 = 1.2;

/// Current chart-view-store schema version stamped into `AppConfig` on
/// every write that touches the store. Read by [`ChartViewStore`] on
/// startup so the reverted-binary scenario (R6) can distinguish a
/// freshly-migrated v2 layout from a brand-new v1 install.
///
/// `CHART_VIEW_STORE_SCHEMA_V1` + `CURRENT_CHART_VIEW_STORE_SCHEMA` are
/// exported for slice 9a's rollback coordination; the writer side
/// uses [`ChartViewStore::schema_version`] to read the stamp, and
/// [`ChartViewStore::set_schema_version`] to seed it from `AppConfig`.
#[allow(dead_code)] // consumed by slice 9a's rollback coordination
pub const CHART_VIEW_STORE_SCHEMA_V1: u32 = 1;
pub const CHART_VIEW_STORE_SCHEMA_V2: u32 = 2;

/// Current schema version this binary writes. Slice 9c flips this to
/// `3` when the v1 writes retire.
#[allow(dead_code)] // consumed by slice 9a's rollback coordination
pub const CURRENT_CHART_VIEW_STORE_SCHEMA: u32 = CHART_VIEW_STORE_SCHEMA_V2;

// ── v2 key primitives ───────────────────────────────────────────────

/// Owned calendar identifier used inside the v2 store key. Mirrors
/// `midas_calendar::CalendarId` (a `&'static str` newtype) without
/// pulling the root-workspace calendar crate in unconditionally —
/// `midas_calendar` is gated on the `session_chart` feature.
///
/// Slice 9c will re-express this as `midas_calendar::CalendarId` once
/// `session_chart` becomes unconditional and the legacy stack is gone.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CalendarIdKey(pub String);

impl CalendarIdKey {
    /// Canonical "no calendar" placeholder used for call sites that
    /// don't yet know the calendar — e.g. a legacy chart panel that
    /// bound a symbol before the resolver fired. The v2 reader treats
    /// `NONE` equivalently to missing so legacy call paths keep
    /// working.
    pub const NONE: &'static str = "NONE";

    /// Wrap a string slice into a calendar key. Trimming + case
    /// normalisation are NOT applied — calendar ids are fixed
    /// identifiers ("XNYS", "CRYPTO", "NONE") and exact-match is part
    /// of the contract.
    #[allow(dead_code)] // consumed by slice 8.5 session-chart rewiring
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Placeholder for call sites without calendar knowledge.
    pub fn none() -> Self {
        Self(Self::NONE.to_string())
    }
}

/// Owned bar-period marker used inside the v2 store key.
///
/// This is intentionally a thin enum rather than a re-export of
/// `midas_bars::BarPeriod` so the store key stays feature-agnostic —
/// `midas_bars` is `session_chart`-gated. A `From<Timeframe>` impl
/// bridges the legacy enum; the session-chart integration will add
/// `From<midas_bars::BarPeriod>` when it lands in slice 8.5 / 9a.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BarPeriodKey {
    /// Clock-interval bar, measured in seconds. Mirrors
    /// `BarPeriod::Clock(ClockInterval::Seconds(_))` without the
    /// dependency.
    Seconds(u32),
    /// Clock-interval bar, measured in minutes.
    Minutes(u32),
    /// Clock-interval bar, measured in hours.
    Hours(u32),
    /// Daily regular-hours session bar. Mirrors
    /// `BarPeriod::Session(SessionSpan::Regular)`.
    DailyRegular,
    /// Daily extended-hours session bar. Mirrors
    /// `BarPeriod::Session(SessionSpan::Extended)`. Consumed by slice
    /// 8.5 callers that drive an ETH chart.
    #[allow(dead_code)] // consumed by slice 8.5 session-chart ETH path
    DailyExtended,
    /// ISO-week calendar bar.
    Week,
    /// Calendar-month bar.
    Month,
}

impl BarPeriodKey {
    /// Translate a legacy [`Timeframe`] into its v2 key form. Kept here
    /// (not on `Timeframe`) so the legacy crate doesn't need to know
    /// about the new key shape. The mapping follows the naming of
    /// `midas_bars::BarPeriod::{m1, m5, d1_rth, …}` so slice 8.5 can
    /// replace `From<Timeframe>` with the real `BarPeriod` without
    /// disturbing the store layout.
    pub fn from_timeframe(tf: Timeframe) -> Self {
        match tf {
            Timeframe::S1 => Self::Seconds(1),
            Timeframe::S5 => Self::Seconds(5),
            Timeframe::S15 => Self::Seconds(15),
            Timeframe::S30 => Self::Seconds(30),
            Timeframe::M1 => Self::Minutes(1),
            Timeframe::M5 => Self::Minutes(5),
            Timeframe::M15 => Self::Minutes(15),
            Timeframe::M30 => Self::Minutes(30),
            Timeframe::H1 => Self::Hours(1),
            Timeframe::H4 => Self::Hours(4),
            Timeframe::D1 => Self::DailyRegular,
            Timeframe::W1 => Self::Week,
            Timeframe::MN1 => Self::Month,
        }
    }
}

impl From<Timeframe> for BarPeriodKey {
    fn from(tf: Timeframe) -> Self {
        Self::from_timeframe(tf)
    }
}

/// Axis-mapping intent stored alongside a v2 view.
///
/// Derived from `ChartPanel.collapse_gaps`. The literal `SessionedTimeAxis`
/// and `ContinuousTimeAxis` types land in slice 10 / Phase F; until then
/// we stash the intent on the view state and let the Phase F caller pick
/// the concrete axis at scene-build time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum AxisIntent {
    /// `collapse_gaps == false` on the owning chart panel → render
    /// with a continuous (wall-clock) time axis. Maps to
    /// `ContinuousTimeAxis` in slice 10.
    #[default]
    Continuous,
    /// `collapse_gaps == true` → compress out-of-session gaps using
    /// the session calendar. Maps to `SessionedTimeAxis(calendar)`
    /// in slice 10.
    Sessioned,
}

impl AxisIntent {
    /// Collapsed-gaps flag round-trip used by `capture_from_camera`.
    pub fn from_collapse_gaps(collapse_gaps: bool) -> Self {
        if collapse_gaps {
            Self::Sessioned
        } else {
            Self::Continuous
        }
    }

    /// Whether the stored intent represents collapsed-gaps rendering.
    /// Consumed by the slice-10 / Phase F scene builder.
    #[allow(dead_code)] // consumed by slice 10 Phase F axis selection
    pub fn is_collapsed(self) -> bool {
        matches!(self, Self::Sessioned)
    }
}

/// Per-(symbol, timeframe) view settings.
#[derive(Clone, Debug, Default)]
pub struct ChartViewState {
    /// Number of candles the user wants visible in the viewport.
    /// `None` = use [`DEFAULT_VISIBLE_CANDLES`].
    visible_candles: Option<usize>,

    /// Price zoom factor: ratio of visible price range to the data's
    /// natural high-low range for the visible candles.
    ///
    /// - `None` = use [`DEFAULT_PRICE_ZOOM`] (auto-fit + padding)
    /// - `1.0` = exactly the data range, no padding
    /// - `< 1.0` = zoomed in (user squeezed Y axis)
    /// - `> 1.0` = zoomed out (user stretched Y axis or has padding)
    price_zoom_factor: Option<f64>,

    /// Axis-mapping intent captured at `capture_from_camera` time.
    /// Mirrors `ChartPanel.collapse_gaps`; the Phase F scene builder
    /// translates this to a concrete `TimeAxis` impl. Default is
    /// [`AxisIntent::Continuous`] for forward-compat with legacy
    /// call sites that never set the flag.
    axis_intent: AxisIntent,
}

impl ChartViewState {
    /// The effective visible candle count.
    pub fn candle_count(&self) -> usize {
        self.visible_candles.unwrap_or(DEFAULT_VISIBLE_CANDLES)
    }

    /// The axis-mapping intent recorded on the last capture. Consumed
    /// by the slice-10 / Phase F scene builder to pick between
    /// `ContinuousTimeAxis` and `SessionedTimeAxis(calendar)`.
    #[allow(dead_code)] // consumed by slice 10 Phase F axis selection
    pub fn axis_intent(&self) -> AxisIntent {
        self.axis_intent
    }

    /// Record the current X and Y zoom levels from a live camera + data.
    ///
    /// **X zoom**: counts visible candles in the camera's time window.
    /// **Y zoom**: computes the ratio of the camera's price range to
    /// the natural data range of those visible candles.
    pub fn capture_from_camera(
        &mut self,
        camera: &Camera2D,
        buf: &CandleBuffer,
        collapse_gaps: bool,
    ) {
        // Axis intent always reflects the owning panel's current
        // `collapse_gaps` flag — captured even when the buffer is
        // empty so the intent round-trips independently of data.
        self.axis_intent = AxisIntent::from_collapse_gaps(collapse_gaps);

        if buf.is_empty() {
            return;
        }

        // X zoom: visible candle count.
        let (vis_start, vis_end) = if collapse_gaps {
            let s = (camera.time_start.floor() as usize).min(buf.len());
            let e = (camera.time_end.ceil() as usize).min(buf.len());
            (s, e)
        } else {
            let s = buf.find_index_by_time(camera.time_start as i64);
            let e = (buf.find_index_by_time(camera.time_end as i64) + 1).min(buf.len());
            (s, e)
        };
        let count = vis_end.saturating_sub(vis_start);
        if count > 0 {
            self.visible_candles = Some(count);
        }

        // Y zoom: price range ratio.
        if vis_start < vis_end {
            let (lo, hi) = buf.price_range(vis_start..vis_end);
            let data_range = (hi - lo) as f64;
            if data_range > 0.0 {
                let camera_range = camera.price_high - camera.price_low;
                if camera_range > 0.0 {
                    self.price_zoom_factor = Some(camera_range / data_range);
                }
            }
        }
    }

    /// Position the camera to show the last N candles at the sweet spot.
    ///
    /// This is the **single authority** for camera positioning on data load.
    /// - **X**: last `candle_count()` candles, last candle at ~87.5%
    /// - **Y**: auto-scaled to visible range, then stretched/squeezed
    ///   by the saved `price_zoom_factor` (preserves user's Y zoom)
    /// - `data_time_start/end`: set for scroll clamping
    pub fn position_camera(
        &self,
        camera: &mut Camera2D,
        buf: &CandleBuffer,
        collapse_gaps: bool,
        data_time_start: &mut f64,
        data_time_end: &mut f64,
    ) {
        if buf.is_empty() {
            return;
        }
        let len = buf.len();

        // Set data bounds for scroll clamping.
        if collapse_gaps {
            *data_time_start = 0.0;
            *data_time_end = len as f64;
        } else {
            *data_time_start = buf.timestamps[0] as f64;
            *data_time_end = buf.timestamps[len - 1] as f64;
        }

        // X positioning.
        let vc = self.candle_count().min(len).max(1);

        if collapse_gaps {
            let start_idx = (len - vc) as f64;
            let data_span = len as f64 - start_idx;
            camera.time_start = start_idx;
            camera.time_end = len as f64 + data_span * RIGHT_PADDING_FRACTION;
        } else {
            let last_ts = buf.timestamps[len - 1] as f64;
            let first_vis = buf.timestamps[len - vc] as f64;
            let data_span = last_ts - first_vis;
            camera.time_start = first_vis;
            camera.time_end = last_ts + data_span * RIGHT_PADDING_FRACTION;
        }

        // Y positioning: center on the last candle's close price,
        // then apply the saved zoom factor to the visible data range.
        let range = (len - vc)..len;
        let (lo, hi) = buf.price_range(range);
        let data_range = (hi - lo) as f64;
        let last_close = buf.closes[len - 1] as f64;

        let factor = self.price_zoom_factor.unwrap_or(DEFAULT_PRICE_ZOOM);
        let visible_range = if data_range > 0.0 {
            data_range * factor
        } else {
            // Flat data (single price) — use a small default range.
            last_close * 0.02
        };

        camera.price_low = last_close - visible_range / 2.0;
        camera.price_high = last_close + visible_range / 2.0;
    }
}

/// Central store for per-(symbol, timeframe) view settings.
///
/// Session-scoped (not persisted to disk). Resets on app restart.
///
/// ## Dual-schema layout (slice 8a)
///
/// [`ChartViewStore`] maintains two independent maps during the
/// chart-transition migration window:
///
/// - `views_v1` keyed on `(String, Timeframe)` — legacy; the only
///   schema a pre-transition binary knows how to read. Writers always
///   update this map so rollback to an older binary preserves user
///   zoom state (plan R6).
/// - `views_v2` keyed on `(String, CalendarIdKey, BarPeriodKey)` —
///   the session-aware shape the new chart stack reads. Carries the
///   `collapse_gaps` → [`AxisIntent`] flag.
///
/// The reader order is v2-first, v1-fallback. A successful v1 lookup
/// **migrates forward**: the value gets re-inserted into v2 on the
/// next write (first-call-wins). See [`ChartViewStore::schema_version`]
/// for the persisted schema marker the `AppConfig.chart_view_store_schema`
/// field stamps.
#[derive(Default, Debug)]
pub struct ChartViewStore {
    /// v1 map — legacy `(symbol, Timeframe)` key. Writers always
    /// update this so an older binary reverted to still observes user
    /// zoom state.
    views_v1: std::collections::HashMap<(String, Timeframe), ChartViewState>,

    /// v2 map — session-aware `(symbol, calendar, period)` key.
    /// Readers prefer this; v1 is a fallback + migration source.
    views_v2: std::collections::HashMap<(String, CalendarIdKey, BarPeriodKey), ChartViewState>,

    /// Schema version stamped on the last successful write. Drives the
    /// `AppConfig.chart_view_store_schema` persistence so rollback
    /// coordination knows what layout the last-running binary used.
    /// Starts at [`CHART_VIEW_STORE_SCHEMA_V1`] until the first v2
    /// write lifts it to [`CHART_VIEW_STORE_SCHEMA_V2`].
    schema_version: u32,

    /// Pending mirror from the last `get_or_default*` call. The next
    /// getter call copies `views_v2[v2_key]` into `views_v1[v1_key]`
    /// so reverted-binary readers (R6) observe the most recent zoom
    /// state. `None` when no mutation is pending; set by
    /// [`Self::ensure_dual_entry`] and drained by
    /// [`Self::sync_v2_to_v1`].
    dirty_mirror: Option<PendingMirror>,
}

/// Key pair recording a pending v2 → v1 mirror. See
/// [`ChartViewStore::dirty_mirror`].
#[derive(Clone, Debug)]
struct PendingMirror {
    v1_key: (String, Timeframe),
    v2_key: (String, CalendarIdKey, BarPeriodKey),
}

impl ChartViewStore {
    /// Current persisted schema stamp. Slice 9c flips the one-shot
    /// forward-migration permanently by removing the v1 writes.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Get or create the view state for a (symbol, timeframe) pair.
    ///
    /// v2 key is derived via `CalendarIdKey::none()` + [`BarPeriodKey::from`]
    /// so every legacy call site keeps working. The calendar-aware
    /// overload [`Self::get_or_default_v2`] is used by new-stack call
    /// sites that know the calendar.
    pub fn get_or_default(&mut self, symbol: &str, tf: Timeframe) -> &mut ChartViewState {
        let symbol_key = symbol.to_uppercase();
        let cal_key = CalendarIdKey::none();
        let period_key = BarPeriodKey::from(tf);
        self.ensure_dual_entry(symbol_key, tf, cal_key, period_key)
    }

    /// Get or create the view state using the explicit v2 key shape.
    /// Slice 8.5 callers that have already resolved the calendar use
    /// this instead of [`Self::get_or_default`]. Both overloads funnel
    /// through the same storage so a legacy writer followed by a
    /// session-chart reader still observes the zoom levels.
    #[allow(dead_code)] // consumed by slice 8.5 session-chart widget rewiring
    pub fn get_or_default_v2(
        &mut self,
        symbol: &str,
        tf: Timeframe,
        calendar: CalendarIdKey,
        period: BarPeriodKey,
    ) -> &mut ChartViewState {
        let symbol_key = symbol.to_uppercase();
        self.ensure_dual_entry(symbol_key, tf, calendar, period)
    }

    /// Get the view state if it exists (read-only).
    ///
    /// Reader order: v2-first (using the `CalendarIdKey::none()` key so
    /// legacy call sites resolve), then v1 fallback. A v1 hit triggers
    /// a forward-migration on the next write via [`Self::get_or_default`].
    pub fn get(&self, symbol: &str, tf: Timeframe) -> Option<&ChartViewState> {
        let symbol_key = symbol.to_uppercase();
        let cal_key = CalendarIdKey::none();
        let period_key = BarPeriodKey::from(tf);

        // v2 first — covers both calendar-aware writers and the
        // forward-migrated legacy entries.
        if let Some(v) = self
            .views_v2
            .get(&(symbol_key.clone(), cal_key, period_key))
        {
            return Some(v);
        }

        // v1 fallback — reverted-binary + pre-migration windows.
        self.views_v1.get(&(symbol_key, tf))
    }

    /// Ensure an entry exists in BOTH v1 and v2 maps and return a
    /// mutable borrow to the v2 copy.
    ///
    /// ## Dual-write semantics
    ///
    /// Callers mutate the returned `&mut ChartViewState` which points
    /// into `views_v2`. On EVERY subsequent getter call the store
    /// runs [`Self::sync_v2_to_v1`] first, which mirrors the latest
    /// v2 state back into `views_v1` so reverted binaries (R6)
    /// observe the latest zoom. The mirror is cheap — one HashMap
    /// lookup + one `Clone` per call.
    ///
    /// The alternative (mirror AFTER the caller drops the `&mut`) is
    /// not expressible without a custom RAII guard; the
    /// sync-on-next-access approach gives the same observable
    /// behaviour for every call site today — they all follow the
    /// two-phase "write through getter, read later" pattern.
    fn ensure_dual_entry(
        &mut self,
        symbol_key: String,
        tf: Timeframe,
        cal_key: CalendarIdKey,
        period_key: BarPeriodKey,
    ) -> &mut ChartViewState {
        // Drain any mirror pending from a previous mutation.
        self.sync_v2_to_v1();

        let v1_key = (symbol_key.clone(), tf);
        let v2_key = (symbol_key, cal_key, period_key);

        // Forward-migrate a v1-only entry into v2 so the reader picks
        // it up on the next fetch. Opportunistic: we only clone if a
        // v1 entry exists and v2 doesn't.
        if !self.views_v2.contains_key(&v2_key) {
            if let Some(v1_state) = self.views_v1.get(&v1_key).cloned() {
                self.views_v2.insert(v2_key.clone(), v1_state);
                tracing::debug!(
                    target: "midas_app::chart_view",
                    symbol = %v1_key.0,
                    ?tf,
                    "forward-migrated v1 ChartViewState into v2 map"
                );
            }
        }

        // Ensure both maps have an entry so later getters see a
        // consistent snapshot — default-constructed on the miss side.
        self.views_v1.entry(v1_key.clone()).or_default();
        self.views_v2.entry(v2_key.clone()).or_default();

        // Eagerly bump the schema stamp on first dual-write. Slice 9c
        // will remove this once v1 stops being written.
        if self.schema_version < CHART_VIEW_STORE_SCHEMA_V2 {
            self.schema_version = CHART_VIEW_STORE_SCHEMA_V2;
        }

        // Record the mirror target so the next getter call picks up
        // whatever the caller writes through the vended `&mut`.
        self.dirty_mirror = Some(PendingMirror {
            v1_key: v1_key.clone(),
            v2_key: v2_key.clone(),
        });

        self.views_v2
            .get_mut(&v2_key)
            .expect("v2 entry was inserted above")
    }

    /// Flush any pending v2 → v1 mirror. No-op when nothing is
    /// pending. Called at the start of every mutating getter and
    /// explicitly by [`Self::force_sync_mirror`] for tests.
    fn sync_v2_to_v1(&mut self) {
        let Some(target) = self.dirty_mirror.take() else {
            return;
        };
        if let Some(v2_state) = self.views_v2.get(&target.v2_key).cloned() {
            self.views_v1.insert(target.v1_key, v2_state);
        }
    }

    /// Test-only hook that flushes the pending mirror. Prod call sites
    /// let it happen lazily on the next getter call.
    #[cfg(test)]
    pub(crate) fn force_sync_mirror(&mut self) {
        self.sync_v2_to_v1();
    }

    /// Install a schema marker read out of `AppConfig` on startup.
    /// Called from the config-load path so the in-memory store
    /// reflects what the previous run persisted. Absence means
    /// "never written" — leave the default v1 stamp.
    pub fn set_schema_version(&mut self, v: u32) {
        self.schema_version = v;
    }

    /// Direct v1-map accessor used by the dual-schema migration tests
    /// to prove that every v2 write also lands in v1. Not part of the
    /// general public API — production code reads via
    /// [`Self::get`] / [`Self::get_or_default`].
    #[cfg(test)]
    pub(crate) fn v1_entry(&self, symbol: &str, tf: Timeframe) -> Option<&ChartViewState> {
        self.views_v1.get(&(symbol.to_uppercase(), tf))
    }

    /// Direct v2-map accessor used by tests to assert the v2 key was
    /// constructed with the expected calendar + period shape.
    #[cfg(test)]
    pub(crate) fn v2_entry(
        &self,
        symbol: &str,
        calendar: &CalendarIdKey,
        period: &BarPeriodKey,
    ) -> Option<&ChartViewState> {
        self.views_v2
            .get(&(symbol.to_uppercase(), calendar.clone(), period.clone()))
    }

    /// Test-only constructor that seeds the store with a legacy v1
    /// entry only. Simulates the "this binary was previously older and
    /// wrote v1-only" migration case.
    #[cfg(test)]
    pub(crate) fn with_v1_seed(symbol: &str, tf: Timeframe, state: ChartViewState) -> Self {
        let mut store = Self::default();
        store.views_v1.insert((symbol.to_uppercase(), tf), state);
        store.schema_version = CHART_VIEW_STORE_SCHEMA_V1;
        store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_candles(count: usize) -> ChartViewState {
        ChartViewState {
            visible_candles: Some(count),
            ..Default::default()
        }
    }

    /// Construct a throwaway `Camera2D` for the axis-intent tests.
    /// `Camera2D` doesn't derive `Default` (`dpi_scale == 0.0` would
    /// violate its invariants) so tests assemble one by hand.
    fn test_camera() -> Camera2D {
        Camera2D {
            time_start: 0.0,
            time_end: 1000.0,
            price_low: 90.0,
            price_high: 110.0,
            viewport_width: 800,
            viewport_height: 400,
            dpi_scale: 1.0,
        }
    }

    // ── R6 rollback — reverted-binary reads v1 entries ─────────────────

    #[test]
    fn reader_prefers_v2_over_v1_when_both_present() {
        // Two different states: v1 has candles=50, v2 has candles=123.
        let mut store = ChartViewStore::default();
        store
            .views_v1
            .insert(("AAPL".into(), Timeframe::M5), state_with_candles(50));
        store.views_v2.insert(
            (
                "AAPL".into(),
                CalendarIdKey::none(),
                BarPeriodKey::from(Timeframe::M5),
            ),
            state_with_candles(123),
        );
        let got = store.get("aapl", Timeframe::M5).expect("present");
        assert_eq!(got.candle_count(), 123, "v2 wins when both exist");
    }

    #[test]
    fn reader_falls_back_to_v1_when_v2_missing() {
        // Simulates "user saved zoom under old binary; new binary is
        // reading before a v2 write has fired."
        let store = ChartViewStore::with_v1_seed("MSFT", Timeframe::D1, state_with_candles(77));
        let got = store.get("msft", Timeframe::D1).expect("v1 hit");
        assert_eq!(got.candle_count(), 77);
    }

    #[test]
    fn v1_writer_to_v2_reader_falls_back() {
        // This is the crucial R6 scenario: v1 writer landed first
        // (legacy binary), then the v2 binary reads it. Behaviour
        // must be "no data loss" — the reader sees the v1 value.
        let mut store = ChartViewStore::default();
        // Simulate v1-only write — direct insert, not through the
        // store API so we exercise the reader's fallback path alone.
        store
            .views_v1
            .insert(("NVDA".into(), Timeframe::H1), state_with_candles(42));

        // Fresh reader call — no v2 entry yet.
        let before = store.get("NVDA", Timeframe::H1).expect("v1 fallback");
        assert_eq!(before.candle_count(), 42);
        // v2 stays untouched because `get` is read-only.
        assert!(store
            .v2_entry(
                "NVDA",
                &CalendarIdKey::none(),
                &BarPeriodKey::from(Timeframe::H1)
            )
            .is_none());
    }

    #[test]
    fn v2_writer_to_v1_reader_sees_same_value() {
        // Reverse: v2 writer (new binary) must mirror into v1 so a
        // downgraded binary reads the same state.
        let mut store = ChartViewStore::default();
        {
            let s = store.get_or_default("TSLA", Timeframe::M15);
            s.visible_candles = Some(88);
        }
        // Mirror is deferred until the next getter call; force it to
        // run synchronously so the direct v1 read below observes the
        // latest v2 state. Production paths don't need this because
        // every subsequent read goes through a getter, which syncs
        // first.
        store.force_sync_mirror();
        let v1 = store.v1_entry("TSLA", Timeframe::M15).expect("v1 mirrored");
        assert_eq!(v1.candle_count(), 88);
    }

    // ── Dual-write semantics ───────────────────────────────────────────

    #[test]
    fn get_or_default_dual_writes_v1_and_v2() {
        let mut store = ChartViewStore::default();
        {
            let s = store.get_or_default("SPY", Timeframe::D1);
            s.visible_candles = Some(300);
            s.price_zoom_factor = Some(1.5);
        }
        store.force_sync_mirror();
        let v1 = store.v1_entry("SPY", Timeframe::D1).expect("v1 present");
        let v2 = store
            .v2_entry(
                "SPY",
                &CalendarIdKey::none(),
                &BarPeriodKey::from(Timeframe::D1),
            )
            .expect("v2 present");
        assert_eq!(v1.candle_count(), v2.candle_count());
        // v1 carries the mirrored value even though the caller only
        // touched v2 (the returned &mut points at v2 by contract).
        assert_eq!(v1.candle_count(), 300);
        assert_eq!(v1.price_zoom_factor, Some(1.5));
    }

    #[test]
    fn first_dual_write_bumps_schema_to_v2() {
        let mut store = ChartViewStore::default();
        assert_eq!(store.schema_version(), 0, "default is 0 (unwritten)");
        let _ = store.get_or_default("AAPL", Timeframe::M1);
        assert_eq!(store.schema_version(), CHART_VIEW_STORE_SCHEMA_V2);
    }

    // ── One-shot forward migration (v1 → v2 on first call) ─────────────

    #[test]
    fn one_shot_forward_migration_on_first_write() {
        // Start with v1-only (simulates startup after binary upgrade).
        let mut store = ChartViewStore::with_v1_seed("COIN", Timeframe::M5, state_with_candles(65));
        // Before any write, v2 is empty.
        assert!(store
            .v2_entry(
                "COIN",
                &CalendarIdKey::none(),
                &BarPeriodKey::from(Timeframe::M5)
            )
            .is_none());

        // First getter call pulls v1 → v2 via the forward-migrate path.
        {
            let _ = store.get_or_default("COIN", Timeframe::M5);
        }

        let v2 = store
            .v2_entry(
                "COIN",
                &CalendarIdKey::none(),
                &BarPeriodKey::from(Timeframe::M5),
            )
            .expect("forward-migrated into v2");
        assert_eq!(
            v2.candle_count(),
            65,
            "forward-migration carries the v1 zoom level"
        );
    }

    #[test]
    fn forward_migration_preserves_price_zoom_factor() {
        let seed = ChartViewState {
            visible_candles: Some(42),
            price_zoom_factor: Some(0.8),
            ..Default::default()
        };
        let mut store = ChartViewStore::with_v1_seed("QQQ", Timeframe::H1, seed);

        let _ = store.get_or_default("QQQ", Timeframe::H1);
        let v2 = store
            .v2_entry(
                "QQQ",
                &CalendarIdKey::none(),
                &BarPeriodKey::from(Timeframe::H1),
            )
            .expect("v2");
        assert_eq!(v2.price_zoom_factor, Some(0.8));
    }

    // ── Axis-intent round-trip (G-3 collapsed-mode mapping) ────────────

    #[test]
    fn collapse_gaps_true_records_sessioned_intent() {
        let mut s = ChartViewState::default();
        assert_eq!(s.axis_intent(), AxisIntent::Continuous);

        // Empty buffer — still captures intent even though the zoom
        // capture bails out.
        let buf = CandleBuffer::new();
        let cam = test_camera();
        s.capture_from_camera(&cam, &buf, /* collapse_gaps */ true);
        assert_eq!(s.axis_intent(), AxisIntent::Sessioned);
        assert!(s.axis_intent().is_collapsed());
    }

    #[test]
    fn collapse_gaps_false_records_continuous_intent() {
        // Pre-seed a "previously collapsed" axis-intent so the
        // capture flip below is observable.
        let mut s = ChartViewState {
            axis_intent: AxisIntent::Sessioned,
            ..Default::default()
        };
        let buf = CandleBuffer::new();
        let cam = test_camera();
        s.capture_from_camera(&cam, &buf, /* collapse_gaps */ false);
        assert_eq!(s.axis_intent(), AxisIntent::Continuous);
        assert!(!s.axis_intent().is_collapsed());
    }

    // ── Schema marker load path ────────────────────────────────────────

    #[test]
    fn set_schema_version_is_additive() {
        let mut store = ChartViewStore::default();
        store.set_schema_version(CHART_VIEW_STORE_SCHEMA_V2);
        assert_eq!(store.schema_version(), CHART_VIEW_STORE_SCHEMA_V2);
        // First get does not regress the marker.
        let _ = store.get_or_default("AAPL", Timeframe::M1);
        assert_eq!(store.schema_version(), CHART_VIEW_STORE_SCHEMA_V2);
    }

    #[test]
    fn bar_period_key_matches_timeframe_mapping() {
        // Integer mappings must stay stable — slice 9c expects these
        // to line up with midas_bars::BarPeriod::{m1, m5, …} by name.
        assert_eq!(BarPeriodKey::from(Timeframe::M1), BarPeriodKey::Minutes(1));
        assert_eq!(BarPeriodKey::from(Timeframe::M5), BarPeriodKey::Minutes(5));
        assert_eq!(BarPeriodKey::from(Timeframe::H1), BarPeriodKey::Hours(1));
        assert_eq!(
            BarPeriodKey::from(Timeframe::D1),
            BarPeriodKey::DailyRegular
        );
        assert_eq!(BarPeriodKey::from(Timeframe::W1), BarPeriodKey::Week);
        assert_eq!(BarPeriodKey::from(Timeframe::MN1), BarPeriodKey::Month);
    }

    // ── Calendar-aware v2 write ────────────────────────────────────────

    #[test]
    fn v2_writer_with_explicit_calendar_separates_from_default() {
        let mut store = ChartViewStore::default();
        let xnys = CalendarIdKey::new("XNYS");

        {
            let s = store.get_or_default_v2(
                "AAPL",
                Timeframe::D1,
                xnys.clone(),
                BarPeriodKey::DailyRegular,
            );
            s.visible_candles = Some(250);
        }
        store.force_sync_mirror();

        // The default-calendar lookup path should NOT find the
        // explicit-calendar entry — they are separate v2 keys.
        assert!(store
            .v2_entry("AAPL", &CalendarIdKey::none(), &BarPeriodKey::DailyRegular)
            .is_none());

        // Explicit-calendar lookup hits.
        let hit = store
            .v2_entry("AAPL", &xnys, &BarPeriodKey::DailyRegular)
            .expect("explicit calendar entry");
        assert_eq!(hit.candle_count(), 250);

        // v1 mirror still uses timeframe only — collapsing calendar
        // → timeframe is the whole point of the migration.
        let v1 = store.v1_entry("AAPL", Timeframe::D1).expect("v1 mirror");
        assert_eq!(v1.candle_count(), 250);
    }
}
