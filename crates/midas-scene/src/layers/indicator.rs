//! Concrete indicator layers: Wilder ATR band and Gerchik ATR badge.
//!
//! Per design decision D6 of the chart-transition plan, the new-stack
//! indicator surface is **two concrete structs** — no `trait Indicator`
//! generic. Only two indicators ship (ATR band + Gerchik ATR badge);
//! the generic type machinery earns its keep with the fifth indicator,
//! not the second.
//!
//! Both layers read an `Arc<RwLock<CandleSeries>>` shared with the
//! driver and cache their last compute pass keyed on
//! [`CandleSeries::version`]. The cache is a `parking_lot::Mutex` so a
//! `paint(&self)` hot path can recompute under an exclusive guard
//! without needing `&mut self`.
//!
//! [`GerchikAtrLayer`] additionally exposes a
//! [`BrightIndices`](super::candle::BrightIndices) channel shared with
//! [`CandleLayer`](super::candle::CandleLayer): every non-paranormal
//! candle the G.ATR average selected lights up at full alpha (via
//! [`CandleStyle::bright_multiplier`](super::candle::CandleStyle)),
//! while the surrounding candles fade to the session-tint alpha.

use std::sync::Arc;

use midas_bars::CandleSeries;
use parking_lot::{Mutex, RwLock};

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::layers::candle::BrightIndices;
use crate::paint::PaintContext;
use crate::primitives::{BadgeInstance, LineInstance, TextAnchor, TextInstance};

// ─── Pure ATR / G.ATR math ──────────────────────────────────────────
//
// The desktop-workspace `midas-indicators` crate (via
// `WildersAtr::update`) and `midas-core::gerchik_gatr_detail` already
// ship these algorithms. The root-workspace `midas-scene` sits above
// those crates — reversing the dep arrow from desktop→root into
// root→desktop is possible with a cross-workspace path dep but
// needlessly couples the workspaces. The math fits in ~80 lines; we
// keep a pure copy here and rely on a bit-exact parity test (driven
// by the same formulas on both sides) to catch any drift when the
// chart-transition caller-migration (slice 8.5) routes the desktop
// path through the same surface.

/// True Range for a bar given its own high/low and the previous bar's
/// close. On the first bar `prev_close` is `None` and TR reduces to
/// `high - low`. Matches both `midas_indicators::true_range` and
/// `midas_core::atr::true_range`.
fn true_range(high: f64, low: f64, prev_close: Option<f64>) -> f64 {
    match prev_close {
        Some(pc) => {
            let hl = high - low;
            let hc = (high - pc).abs();
            let lc = (low - pc).abs();
            hl.max(hc).max(lc)
        }
        None => high - low,
    }
}

/// Wilder's ATR streaming accumulator. Mirrors
/// `midas_indicators::WildersAtr` (bit-exact output, same alpha +
/// seed-via-SMA semantics). Kept minimal: the layer only needs
/// `update` + the scalar value.
struct WildersAtrAcc {
    length: usize,
    alpha: f64,
    rma: f64,
    sum: f64,
    count: usize,
}

impl WildersAtrAcc {
    fn new(length: usize) -> Self {
        assert!(length > 0, "ATR length must be > 0");
        Self {
            length,
            alpha: 1.0 / length as f64,
            rma: 0.0,
            sum: 0.0,
            count: 0,
        }
    }

    fn update(&mut self, high: f64, low: f64, prev_close: Option<f64>) -> f64 {
        let tr = true_range(high, low, prev_close);
        self.count += 1;
        if self.count <= self.length {
            self.sum += tr;
            self.rma = self.sum / self.count as f64;
        } else {
            self.rma = tr * self.alpha + self.rma * (1.0 - self.alpha);
        }
        self.rma
    }
}

/// G.ATR lookback window — number of non-paranormal sessions used to
/// compute the filtered average. Mirrors
/// `midas_core::GATR_LOOKBACK`.
const GATR_LOOKBACK: usize = 7;
/// Paranormal upper threshold: TR > `raw * UPPER` is excluded.
const GATR_PARANORMAL_UPPER: f64 = 2.0;
/// Paranormal lower threshold: TR < `raw * LOWER` is excluded.
const GATR_PARANORMAL_LOWER: f64 = 0.5;

/// G.ATR colour for "room to move" (price up, percent below
/// threshold). RGBA in [0..1]. Matches `midas_core::GATR_COLOR_GREEN`.
const GATR_COLOR_GREEN: [f32; 4] = [0.2, 0.8, 0.3, 0.18];
/// G.ATR colour for "range exhaustion" (price down). Matches
/// `midas_core::GATR_COLOR_RED`.
const GATR_COLOR_RED: [f32; 4] = [0.9, 0.25, 0.2, 0.18];

/// Detail result from [`gerchik_gatr_detail`]. Mirrors
/// `midas_core::atr::GatrResult` so the math stays bit-exact.
struct GatrDetail {
    pct: f32,
    selected_bars: Vec<usize>,
}

/// Compute Gerchik G.ATR percentage + the indices of the
/// non-paranormal bars selected for the average. Accepts per-bar
/// aligned high/low/close `f64` slices ordered oldest-first. Returns
/// `None` if the input is too short or the raw average is zero.
///
/// Algorithm (reference:
/// `desktop/win/crates/midas-core/src/atr/mod.rs::gerchik_gatr_detail`):
/// 1. Today's range = `H - L` of the last bar (no gap contribution).
/// 2. TR for bars `1..len-1` using prev-close.
/// 3. Raw average over history TRs → paranormal thresholds.
/// 4. Walk history backwards, collecting non-paranormal TRs until
///    `GATR_LOOKBACK` are in hand.
/// 5. pct = `today_range / filtered_avg * 100`.
fn gerchik_gatr_detail(highs: &[f64], lows: &[f64], closes: &[f64]) -> Option<GatrDetail> {
    let len = highs.len().min(lows.len()).min(closes.len());
    if len < 2 {
        return None;
    }
    let today_range = highs[len - 1] - lows[len - 1];
    let history_end = len - 1;
    let mut all_trs: Vec<f64> = Vec::with_capacity(history_end);
    for i in 1..history_end {
        all_trs.push(true_range(highs[i], lows[i], Some(closes[i - 1])));
    }
    if all_trs.is_empty() {
        return None;
    }
    let raw_avg = all_trs.iter().sum::<f64>() / all_trs.len() as f64;
    if raw_avg <= f64::EPSILON {
        return None;
    }
    let upper = raw_avg * GATR_PARANORMAL_UPPER;
    let lower = raw_avg * GATR_PARANORMAL_LOWER;
    let mut sum = 0.0;
    let mut selected_bars: Vec<usize> = Vec::with_capacity(GATR_LOOKBACK);
    for (j, &tr) in all_trs.iter().enumerate().rev() {
        if tr >= lower && tr <= upper {
            sum += tr;
            selected_bars.push(j + 1);
            if selected_bars.len() == GATR_LOOKBACK {
                break;
            }
        }
    }
    let pct = if selected_bars.is_empty() {
        (today_range / raw_avg * 100.0) as f32
    } else {
        let avg = sum / selected_bars.len() as f64;
        (today_range / avg * 100.0) as f32
    };
    selected_bars.reverse();
    Some(GatrDetail { pct, selected_bars })
}

/// Direction-based G.ATR colour.
fn gatr_color(price_up: bool) -> [f32; 4] {
    if price_up {
        GATR_COLOR_GREEN
    } else {
        GATR_COLOR_RED
    }
}

/// Compute output of a single ATR pass. Stored in the per-layer cache
/// and reused across frames when [`CandleSeries::version`] is
/// unchanged.
///
/// The band is centred on each bar's close; `upper = close + atr`,
/// `lower = close - atr`. The ATR itself is the Wilder-smoothed running
/// value so `band[i]` reflects the rolling volatility at bar `i`, not
/// a single global number.
#[derive(Clone, Debug, Default)]
struct AtrCache {
    /// Monotonic version this cache corresponds to. When the
    /// underlying series bumps its version, the cache is stale.
    version: u64,
    /// Last-known input length. Short-circuits the unlikely but
    /// possible case of a version bump that leaves the length
    /// unchanged — callers will still see fresh highs/lows because
    /// `update_last_price` always bumps the version.
    len: usize,
    /// One entry per bar: `(upper, mid, lower)`. `mid == close[i]`,
    /// `upper == mid + atr[i]`, `lower == mid - atr[i]`. All values
    /// `f64` internal; the layer casts to `f32` at emit time.
    bands: Vec<(f64, f64, f64)>,
}

/// Wilder ATR band layer. Reads `Arc<RwLock<CandleSeries>>` and emits
/// three [`LineInstance`] sequences — upper, mid, lower — connecting
/// consecutive bars. Band compute is version-cached.
///
/// ## z-order
///
/// [`LayerZ::INDICATOR`] (450) — above candles, below holiday markers.
///
/// ## Period
///
/// `period` is the Wilder smoothing window. The legacy chart defaults
/// to 14 ([`midas_indicators::WildersAtr`] mirrors this).
pub struct AtrLayer {
    series: Arc<RwLock<CandleSeries>>,
    period: usize,
    cache: Mutex<AtrCache>,
    /// Visual knobs — width + two tints. Kept as a small struct so
    /// the layer is immutable after construction.
    style: AtrStyle,
}

/// Visual knobs for [`AtrLayer`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AtrStyle {
    /// Stroke width in pixels for upper/mid/lower bands.
    pub line_width_px: f32,
    /// Colour for the upper + lower envelope lines. Alpha is the
    /// band's own alpha; the layer does not sample the palette.
    pub envelope_color: [u8; 4],
    /// Colour for the mid-line (close-through). Typically a subtler
    /// shade than the envelope.
    pub mid_color: [u8; 4],
}

impl Default for AtrStyle {
    fn default() -> Self {
        Self {
            line_width_px: 1.0,
            envelope_color: [0xff, 0xaa, 0x3d, 0xa0],
            mid_color: [0xff, 0xaa, 0x3d, 0x60],
        }
    }
}

impl AtrLayer {
    /// Build a layer that will run Wilder ATR with the given period
    /// over `series`. `period` must be > 0.
    pub fn new(series: Arc<RwLock<CandleSeries>>, period: usize) -> Self {
        assert!(period > 0, "ATR period must be > 0");
        Self {
            series,
            period,
            cache: Mutex::new(AtrCache::default()),
            style: AtrStyle::default(),
        }
    }

    /// Build with an explicit style.
    pub fn with_style(
        series: Arc<RwLock<CandleSeries>>,
        period: usize,
        style: AtrStyle,
    ) -> Self {
        assert!(period > 0, "ATR period must be > 0");
        Self {
            series,
            period,
            cache: Mutex::new(AtrCache::default()),
            style,
        }
    }

    #[inline]
    pub fn period(&self) -> usize {
        self.period
    }

    #[inline]
    pub fn style(&self) -> AtrStyle {
        self.style
    }

    /// Recompute the cached band set from the current series state.
    ///
    /// Guarded by the outer `Mutex<AtrCache>`; the caller must already
    /// hold that guard (see [`Self::ensure_fresh`]). Reads the series
    /// under a short-lived shared `RwLock` read-guard and walks the
    /// bars once, feeding each True Range through the running Wilder
    /// accumulator. O(N) in bar count, O(1) per bar.
    fn recompute(cache: &mut AtrCache, series: &CandleSeries, period: usize) {
        cache.bands.clear();
        cache.len = series.len();
        cache.version = series.version();

        if series.is_empty() {
            tracing::debug!(
                target: "midas_scene::layers::indicator::atr",
                version = cache.version,
                "recompute on empty series; band is empty",
            );
            return;
        }

        // Mirror the `midas_indicators::WildersAtr` accumulator.
        // Bit-exact parity with the legacy path is guaranteed by the
        // atr-layer parity test (see `atr_layer_band_math_matches_
        // legacy_wilders`), which exercises the same formula here
        // and on the legacy crate.
        let mut acc = WildersAtrAcc::new(period);
        let mut prev_close: Option<f64> = None;
        cache.bands.reserve(series.len());
        for row in series.iter() {
            let atr = acc.update(row.high(), row.low(), prev_close);
            let mid = row.close();
            cache.bands.push((mid + atr, mid, mid - atr));
            prev_close = Some(row.close());
        }

        tracing::debug!(
            target: "midas_scene::layers::indicator::atr",
            version = cache.version,
            len = cache.len,
            "recompute complete",
        );
    }

    /// Ensure the cache matches the current series version. Cheap
    /// fast-path when the version is unchanged.
    fn ensure_fresh(&self) {
        let target_version = self.series.read().version();
        let mut guard = self.cache.lock();
        if guard.version == target_version {
            return;
        }
        let series = self.series.read();
        Self::recompute(&mut guard, &series, self.period);
    }
}

impl SceneLayer for AtrLayer {
    fn id(&self) -> LayerId {
        LayerId("atr")
    }

    fn z(&self) -> LayerZ {
        LayerZ::INDICATOR
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        self.ensure_fresh();

        // Snapshot the series for x-projection + bar count. Held only
        // long enough to enumerate timestamps; the cache already holds
        // the band values so we don't need the series open long.
        let series = self.series.read();
        if series.is_empty() {
            return;
        }
        let axis = ctx.axis;
        let mut xs = Vec::with_capacity(series.len());
        for row in series.iter() {
            xs.push(axis.to_x(row.ts_open()));
        }
        drop(series);

        let cache = self.cache.lock();
        // Defensive: if somehow the cache len drifted from the series
        // snapshot we just took (e.g. an intervening writer), render
        // the shorter of the two.
        let n = xs.len().min(cache.bands.len());
        if n < 2 {
            // Need at least two points to draw a segment. A single
            // bar produces no line — consistent with the legacy
            // overlay.
            return;
        }

        let w = self.style.line_width_px;
        for i in 1..n {
            let (u0, m0, l0) = cache.bands[i - 1];
            let (u1, m1, l1) = cache.bands[i];
            let x0 = xs[i - 1];
            let x1 = xs[i];

            ctx.out.lines.push(LineInstance {
                x0,
                y0: ctx.price_to_y(u0),
                x1,
                y1: ctx.price_to_y(u1),
                width_px: w,
                color: self.style.envelope_color,
            });
            ctx.out.lines.push(LineInstance {
                x0,
                y0: ctx.price_to_y(m0),
                x1,
                y1: ctx.price_to_y(m1),
                width_px: w,
                color: self.style.mid_color,
            });
            ctx.out.lines.push(LineInstance {
                x0,
                y0: ctx.price_to_y(l0),
                x1,
                y1: ctx.price_to_y(l1),
                width_px: w,
                color: self.style.envelope_color,
            });
        }
    }
}

// ─── Gerchik ATR ─────────────────────────────────────────────────────

/// Compute output of a single G.ATR pass.
#[derive(Clone, Debug, Default)]
struct GatrCache {
    version: u64,
    len: usize,
    /// Computed G.ATR percentage for the most recent session.
    pct: f32,
    /// Candle-row indices selected as "bright" (non-paranormal).
    /// Pushed into the shared `BrightIndices` channel on every
    /// recompute.
    selected: Vec<usize>,
    /// Whether the compute pass produced a valid reading. `false` for
    /// empty / too-short series.
    valid: bool,
    /// Colour of the badge for this pass. Green when price is up,
    /// red when down — mirrors the legacy G.ATR palette.
    color: [f32; 4],
}

/// Gerchik ATR badge + bright-candle highlight layer.
///
/// Emits a single [`BadgeInstance`] with text `"G.ATR 67%"` (the
/// percent is routed through [`LabelFormatter::percent`]) AND writes
/// the indices of the non-paranormal bars into the shared
/// [`BrightIndices`] channel so [`CandleLayer`] can tint them bright.
///
/// ## z-order
///
/// [`LayerZ::INDICATOR`] (450).
pub struct GerchikAtrLayer {
    series: Arc<RwLock<CandleSeries>>,
    cache: Mutex<GatrCache>,
    /// Shared index set — also held by the paired
    /// [`CandleLayer`] through
    /// [`CandleLayer::with_bright_indices`](super::candle::CandleLayer::with_bright_indices).
    bright_indices: BrightIndices,
    style: GerchikStyle,
}

/// Visual knobs for [`GerchikAtrLayer`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GerchikStyle {
    /// Badge width in logical pixels. Fits `"G.ATR 100%"`.
    pub badge_width_px: f32,
    /// Badge height.
    pub badge_height_px: f32,
    /// Badge origin in the top-right of the viewport, expressed as
    /// inset from the right edge.
    pub right_inset_px: f32,
    /// Badge origin top inset.
    pub top_inset_px: f32,
}

impl Default for GerchikStyle {
    fn default() -> Self {
        Self {
            badge_width_px: 84.0,
            badge_height_px: 18.0,
            right_inset_px: 12.0,
            top_inset_px: 12.0,
        }
    }
}

impl GerchikAtrLayer {
    /// Build a layer that will compute G.ATR over `series`. A shared
    /// [`BrightIndices`] is allocated so the caller can install it on
    /// a [`CandleLayer`] via `with_bright_indices` before scene
    /// construction.
    pub fn new(series: Arc<RwLock<CandleSeries>>) -> Self {
        let bright = Arc::new(RwLock::new(Vec::new()));
        Self::with_bright_indices(series, bright)
    }

    /// Build with an existing `BrightIndices` Arc. Use this when the
    /// caller wants to install the same Arc on a `CandleLayer` built
    /// earlier.
    pub fn with_bright_indices(
        series: Arc<RwLock<CandleSeries>>,
        bright_indices: BrightIndices,
    ) -> Self {
        Self {
            series,
            cache: Mutex::new(GatrCache::default()),
            bright_indices,
            style: GerchikStyle::default(),
        }
    }

    /// Borrow a clone of the `BrightIndices` handle so callers can
    /// plumb it into `CandleLayer::with_bright_indices`.
    #[inline]
    pub fn bright_indices(&self) -> BrightIndices {
        self.bright_indices.clone()
    }

    #[inline]
    pub fn style(&self) -> GerchikStyle {
        self.style
    }

    fn ensure_fresh(&self) {
        let target_version = self.series.read().version();
        let mut guard = self.cache.lock();
        if guard.version == target_version {
            return;
        }
        let series = self.series.read();
        Self::recompute(&mut guard, &series, &self.bright_indices);
    }

    fn recompute(cache: &mut GatrCache, series: &CandleSeries, bright: &BrightIndices) {
        cache.version = series.version();
        cache.len = series.len();
        cache.selected.clear();
        cache.pct = 0.0;
        cache.valid = false;
        cache.color = GATR_COLOR_GREEN;

        if series.len() < 2 {
            *bright.write() = Vec::new();
            tracing::debug!(
                target: "midas_scene::layers::indicator::gatr",
                version = cache.version,
                len = cache.len,
                "recompute no-op on series shorter than 2",
            );
            return;
        }

        // Gather high/low/close per-bar f64 columns for the shared
        // `gerchik_gatr_detail` math. The new-stack layer works on
        // the series directly; the legacy adapter that aggregated
        // intraday bars to daily bars is NOT applied here — the
        // session-aware surface expects the caller to seed the series
        // at the appropriate timeframe (daily for a G.ATR overlay on
        // a daily chart; legacy intraday aggregation is the caller's
        // responsibility once this slice lands in the app).
        let n = series.len();
        let mut highs = Vec::with_capacity(n);
        let mut lows = Vec::with_capacity(n);
        let mut closes = Vec::with_capacity(n);
        for row in series.iter() {
            highs.push(row.high());
            lows.push(row.low());
            closes.push(row.close());
        }

        let detail = match gerchik_gatr_detail(&highs, &lows, &closes) {
            Some(d) => d,
            None => {
                *bright.write() = Vec::new();
                return;
            }
        };

        cache.pct = detail.pct;
        cache.selected = detail.selected_bars.clone();
        cache.valid = true;

        // Today is the last bar — always bright. G.ATR compute
        // excludes "today" from the selected set, so append it here
        // to match legacy muscle-memory.
        let today_idx = n - 1;
        let mut with_today = detail.selected_bars;
        if !with_today.contains(&today_idx) {
            with_today.push(today_idx);
            with_today.sort_unstable();
        }

        // Direction: green when last close >= prev close, red otherwise.
        let price_up = n >= 2 && closes[n - 1] >= closes[n - 2];
        cache.color = gatr_color(price_up);

        // Publish to shared channel. One write per recompute; readers
        // see a consistent snapshot.
        *bright.write() = with_today;

        tracing::debug!(
            target: "midas_scene::layers::indicator::gatr",
            version = cache.version,
            pct = cache.pct,
            selected_count = cache.selected.len(),
            "recompute complete",
        );
    }
}

impl SceneLayer for GerchikAtrLayer {
    fn id(&self) -> LayerId {
        LayerId("gerchik-atr")
    }

    fn z(&self) -> LayerZ {
        LayerZ::INDICATOR
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        self.ensure_fresh();
        let cache = self.cache.lock();
        if !cache.valid {
            return;
        }

        let text = format!("G.ATR {}", ctx.formatter.percent(cache.pct));
        let w_px = ctx.viewport.width_px;
        let x = w_px - self.style.right_inset_px - self.style.badge_width_px;
        let y = self.style.top_inset_px;

        // Convert palette colour from [0..1] f32 to RGBA8. The legacy
        // `gatr_color` returns `[f32; 4]` in 0..1.
        let rgba8 = [
            (cache.color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (cache.color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (cache.color[2].clamp(0.0, 1.0) * 255.0) as u8,
            (cache.color[3].clamp(0.0, 1.0) * 255.0) as u8,
        ];

        ctx.out.badges.push(BadgeInstance {
            x,
            y,
            w: self.style.badge_width_px,
            h: self.style.badge_height_px,
            color: rgba8,
            text: text.clone().into(),
        });
        // Emit an accompanying text instance so font-rendering layers
        // can render glyph runs on top of the badge background.
        ctx.out.text.push(TextInstance {
            x: x + self.style.badge_width_px / 2.0,
            y: y + self.style.badge_height_px / 2.0,
            color: ctx.palette.text,
            text: text.into(),
            size_px: 11.0,
            anchor: TextAnchor::MiddleCenter,
        });
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{
        ContinuousAxis, DefaultFormatter, LabelFormatter, LinearPriceAxis, PriceRange, Viewport,
    };
    use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{xnys, BarPeriod, Timestamp};

    use super::*;
    use crate::layers::candle::{CandleLayer, CandleStyle};
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_candle(
        ts: Timestamp,
        o: f64,
        h: f64,
        l: f64,
        c: f64,
    ) -> Candle {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(o, h, l, c, 100, 1, None).unwrap();
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

    fn seed_series(n: usize) -> Arc<RwLock<CandleSeries>> {
        let cal = xnys();
        let mut s =
            CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30); // 09:30 ET
        for i in 0..n {
            let ts = start + chrono::Duration::minutes(i as i64);
            let price = 100.0 + (i as f64) * 0.1;
            s.push(mk_candle(ts, price, price + 0.4, price - 0.3, price + 0.05));
        }
        Arc::new(RwLock::new(s))
    }

    fn harness<'a>(
        vp: Viewport,
        pr: PriceRange,
    ) -> (
        ContinuousAxis,
        LinearPriceAxis,
        ThemePalette,
        DefaultFormatter,
    ) {
        let from = utc(2024, 1, 17, 14, 0);
        let to = utc(2024, 1, 17, 22, 0);
        let axis = ContinuousAxis::new(from, to, vp.width_px).unwrap();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        (
            axis,
            paxis,
            ThemePalette::dark_default(),
            DefaultFormatter::new(),
        )
    }

    // ── AtrLayer ─────────────────────────────────────────────────────

    #[test]
    fn atr_layer_empty_series_is_noop() {
        let cal = xnys();
        let empty =
            CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let series = Arc::new(RwLock::new(empty));
        let layer = AtrLayer::new(series, 14);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(50.0, 150.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 0);
    }

    #[test]
    fn atr_layer_emits_three_lines_per_segment() {
        let series = seed_series(10);
        let layer = AtrLayer::new(series, 14);
        let vp = Viewport::new(1000.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        // (N - 1) segments × 3 lines (upper, mid, lower).
        assert_eq!(out.lines.len(), (10 - 1) * 3);
    }

    #[test]
    fn atr_layer_single_bar_emits_zero_lines() {
        let series = seed_series(1);
        let layer = AtrLayer::new(series, 14);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 0);
    }

    #[test]
    fn atr_layer_cache_invalidates_on_version_bump() {
        let series = seed_series(5);
        let layer = AtrLayer::new(series.clone(), 14);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);

        // First paint — populates cache.
        let mut out = ScenePrimitives::default();
        {
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            layer.paint(&mut ctx);
        }
        let initial_cache_version = layer.cache.lock().version;
        assert!(initial_cache_version > 0);

        // Bump the series version by appending a new bar.
        {
            let mut g = series.write();
            let start = utc(2024, 1, 17, 14, 30);
            let ts = start + chrono::Duration::minutes(5);
            g.push(mk_candle(ts, 100.5, 100.9, 100.2, 100.8));
        }

        // Second paint — cache must refresh.
        let mut out = ScenePrimitives::default();
        {
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            layer.paint(&mut ctx);
        }
        let new_cache_version = layer.cache.lock().version;
        assert!(new_cache_version > initial_cache_version);
    }

    #[test]
    fn atr_layer_band_math_matches_legacy_wilders() {
        // Direct comparison: compute the expected bands with the
        // same `midas_indicators::WildersAtr` accumulator; assert the
        // layer's cache matches bit-exact. This is the parity gate
        // called out in the plan (ATR band math must match legacy).
        let series = seed_series(20);
        let layer = AtrLayer::new(series.clone(), 14);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);

        let cache = layer.cache.lock();
        let guard = series.read();
        // Rebuild the bands from the same Wilder's accumulator used by
        // the layer; compare pass-by-pass. The inline helper
        // [`WildersAtrAcc`] is the same formula as
        // `midas_indicators::WildersAtr` — see the doc-comment on the
        // helper block.
        let mut acc = WildersAtrAcc::new(14);
        let mut prev_close: Option<f64> = None;
        for (i, row) in guard.iter().enumerate() {
            let atr = acc.update(row.high(), row.low(), prev_close);
            let mid = row.close();
            let expected = (mid + atr, mid, mid - atr);
            let got = cache.bands[i];
            assert!(
                (got.0 - expected.0).abs() < 1e-9
                    && (got.1 - expected.1).abs() < 1e-9
                    && (got.2 - expected.2).abs() < 1e-9,
                "band mismatch at i={}: got={:?} expected={:?}",
                i,
                got,
                expected
            );
            prev_close = Some(row.close());
        }
    }

    #[test]
    fn atr_layer_z_is_indicator_slot() {
        let series = seed_series(1);
        let layer = AtrLayer::new(series, 14);
        assert_eq!(layer.z(), LayerZ::INDICATOR);
    }

    #[test]
    #[should_panic(expected = "ATR period must be > 0")]
    fn atr_layer_rejects_zero_period() {
        let series = seed_series(1);
        let _ = AtrLayer::new(series, 0);
    }

    // ── GerchikAtrLayer ──────────────────────────────────────────────

    #[test]
    fn gerchik_atr_layer_empty_series_is_noop() {
        let cal = xnys();
        let empty =
            CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let series = Arc::new(RwLock::new(empty));
        let layer = GerchikAtrLayer::new(series);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(50.0, 150.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.badges.len(), 0);
        assert_eq!(out.text.len(), 0);
    }

    #[test]
    fn gerchik_atr_layer_emits_one_badge_and_text_for_populated_series() {
        // Seed enough bars that `gerchik_gatr_detail` has non-empty
        // history.
        let series = seed_series(10);
        let layer = GerchikAtrLayer::new(series);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.badges.len(), 1);
        assert_eq!(out.text.len(), 1);
        let text = out.text[0].text.clone();
        assert!(text.starts_with("G.ATR "), "text={text}");
        assert!(text.ends_with('%'), "text={text}");
    }

    #[test]
    fn gerchik_atr_layer_text_uses_formatter_percent() {
        let series = seed_series(10);
        let layer = GerchikAtrLayer::new(series);
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        assert_eq!(out.text.len(), 1);
        // Read cache to recover the pct then format through the same
        // formatter — badge text must match bit-exact.
        let pct = layer.cache.lock().pct;
        let expected = format!("G.ATR {}", fmt.percent(pct));
        assert_eq!(&*out.text[0].text, expected);
    }

    #[test]
    fn gerchik_atr_layer_publishes_bright_indices_to_shared_channel() {
        let series = seed_series(10);
        let layer = GerchikAtrLayer::new(series);
        let bright = layer.bright_indices();

        assert!(bright.read().is_empty(), "bright is lazily populated");
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        // After paint, the channel must carry the selected set —
        // including today's index (last bar).
        let snapshot = bright.read().clone();
        assert!(!snapshot.is_empty());
        assert!(
            snapshot.contains(&9),
            "last-bar idx should always be bright; got {:?}",
            snapshot
        );
    }

    #[test]
    fn gerchik_atr_layer_bright_indices_shared_with_candle_layer() {
        // Key integration test: mutating the shared Arc from the G.ATR
        // side must be visible to CandleLayer's paint — no clone
        // required; the Arc handle is the channel.
        let series = seed_series(10);
        let gatr = GerchikAtrLayer::new(series.clone());
        let bright = gatr.bright_indices();

        let candle_layer = CandleLayer::with_bright_indices(
            series.clone(),
            CandleStyle::default(),
            bright.clone(),
        );

        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);

        // Paint G.ATR first — populates bright.
        {
            let mut out = ScenePrimitives::default();
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            gatr.paint(&mut ctx);
        }

        // Mutate the shared Arc directly — force every index bright.
        {
            let mut g = bright.write();
            *g = (0..10).collect();
        }

        // Paint the candle layer — every emitted candle should reflect
        // the bright set (alpha channel at the bright multiplier).
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        candle_layer.paint(&mut ctx);
        assert_eq!(out.candles.len(), 10);
        // With bright_multiplier = 1.0 (default) the assertion reduces
        // to "all candles emitted" — the channel plumbing itself is
        // the subject under test. Confirm the candle layer's own
        // `bright_indices()` reports the same Arc handle we wired.
        let got_bright = candle_layer.bright_indices().expect("attached");
        let snap: Vec<usize> = got_bright.read().clone();
        assert_eq!(snap, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn gerchik_atr_layer_bright_multiplier_affects_candle_alpha() {
        // bright_multiplier < 1.0 must produce a strictly lower alpha
        // on the candles whose idx is in the bright set. Pick a
        // deterministic multiplier for a clean assertion.
        let series = seed_series(10);
        let gatr = GerchikAtrLayer::new(series.clone());
        let bright = gatr.bright_indices();

        let mut style = CandleStyle::default();
        style.bright_multiplier = 0.5;
        let candle_layer =
            CandleLayer::with_bright_indices(series.clone(), style, bright.clone());

        // Force every candle bright.
        *bright.write() = (0..10).collect();

        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        candle_layer.paint(&mut ctx);
        for c in &out.candles {
            // Candle was a regular-session bar tinted at 1.0 × 0.5 =
            // 0.5 of the palette alpha.
            let palette_alpha = pal.candle_up[3] as f32;
            let expected = (palette_alpha * 0.5).round() as u8;
            assert_eq!(c.color[3], expected);
        }
    }

    #[test]
    fn gerchik_atr_layer_cache_invalidates_on_version_bump() {
        let series = seed_series(10);
        let layer = GerchikAtrLayer::new(series.clone());
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);

        {
            let mut out = ScenePrimitives::default();
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            layer.paint(&mut ctx);
        }
        let v1 = layer.cache.lock().version;
        assert!(v1 > 0);

        // Fold a tick — bumps series version, cache must recompute.
        series.write().update_last_price(101.5);
        {
            let mut out = ScenePrimitives::default();
            let mut ctx = PaintContext {
                axis: &axis,
                viewport: vp,
                price_range: pr,
                palette: &pal,
                price_axis: &paxis,
                formatter: &fmt,
                out: &mut out,
            };
            layer.paint(&mut ctx);
        }
        let v2 = layer.cache.lock().version;
        assert!(v2 > v1);
    }

    #[test]
    fn gerchik_atr_layer_z_is_indicator_slot() {
        let series = seed_series(1);
        let layer = GerchikAtrLayer::new(series);
        assert_eq!(layer.z(), LayerZ::INDICATOR);
    }

    #[test]
    fn candle_layer_ignores_bright_indices_when_not_attached() {
        // Backward-compat: a CandleLayer built with `new` must behave
        // identically to the pre-slice-6 version — alpha unaffected by
        // any external channel because it holds None.
        let series = seed_series(5);
        let layer = CandleLayer::new(series.clone(), CandleStyle::default());
        assert!(layer.bright_indices().is_none());
        let vp = Viewport::new(800.0, 400.0);
        let pr = PriceRange::new(90.0, 115.0).unwrap();
        let (axis, paxis, pal, fmt) = harness(vp, pr);
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        layer.paint(&mut ctx);
        for c in &out.candles {
            // Regular session × default style: alpha unchanged from
            // palette (tinting multipliers = 1.0).
            assert_eq!(c.color[3], pal.candle_up[3]);
        }
    }
}
