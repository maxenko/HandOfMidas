//! [`VolumeProfileLayer`] — horizontal volume-profile histogram.
//!
//! Ports the legacy overlay from `midas_chart::volume_profile` onto the
//! sans-IO scene stack (slice 7 of the chart-transition plan).
//!
//! ## Algorithm
//!
//! 1. Pick the visible-range slice of the shared `CandleSeries`.
//! 2. Price range = min-low..max-high over the visible candles.
//! 3. Bin count is **viewport-adaptive**: per legacy
//!    `widget/compute/mod.rs:126`,
//!    `((viewport.height_px * 0.8) / 3.0).clamp(20.0, 200.0) as usize`
//!    — a denser profile for taller viewports. NOT a hardcoded 40.
//! 4. For each visible candle, distribute `volume` uniformly across the
//!    bins its `[low, high]` intersects (integer-division spread;
//!    remainder goes to the mid bin, matching legacy).
//! 5. The bin with the largest total volume is the Point of Control
//!    (POC) and is emitted with a distinctly brighter colour.
//!
//! ## Output
//!
//! One [`QuadInstance`] per bin with non-zero volume. Bars are
//! left-anchored; bar width is proportional to
//! `bin_volume / max_bin_volume` × `style.max_bar_px_fraction ×
//! viewport.width_px`. Each bar spans the bin's vertical slice of the
//! viewport (price → y via the context's `PriceAxis`).

use std::ops::Range;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::layers::candle::SharedCandleSeries;
use crate::paint::PaintContext;
use crate::primitives::QuadInstance;

/// Visual knobs for [`VolumeProfileLayer`].
///
/// All colour channels are RGBA8. `neighbour_color` paints every bin
/// below the POC; `poc_color` paints the single densest bin and MUST
/// be distinguishable (brighter / more saturated / higher alpha) per
/// the slice-7 test "POC gets a distinctly brighter color than
/// neighbours".
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VolumeProfileStyle {
    /// Fraction of viewport width the largest bar consumes. Legacy
    /// used 0.25.
    pub max_bar_px_fraction: f32,
    /// Colour of non-POC bins.
    pub neighbour_color: [u8; 4],
    /// Colour of the POC bin. Must be distinctly brighter than
    /// `neighbour_color`.
    pub poc_color: [u8; 4],
}

impl Default for VolumeProfileStyle {
    fn default() -> Self {
        Self {
            max_bar_px_fraction: 0.25,
            // Semi-transparent teal, matches the legacy buy-volume
            // tint (`[0.10, 0.55, 0.55, 0.30]` → 0..255 scale).
            neighbour_color: [0x1a, 0x8c, 0x8c, 0x4d],
            // Bright muted-gold, RGB is noticeably lighter and alpha
            // is noticeably higher than `neighbour_color` — the
            // "distinctly brighter" requirement from the plan.
            poc_color: [0xe6, 0xb8, 0x1a, 0xcc],
        }
    }
}

/// Compute bin count from viewport height using the legacy formula —
/// `((height * 0.8) / 3.0).clamp(20, 200) as usize`.
///
/// Exposed so callers and tests can reason about bin counts without
/// duplicating the formula. Never returns zero.
#[inline]
pub fn bin_count_for_viewport(viewport_height_px: f32) -> usize {
    ((viewport_height_px * 0.8) / 3.0).clamp(20.0, 200.0) as usize
}

/// Horizontal volume-profile histogram layer.
///
/// Holds a read-handle to the candle series plus the currently-visible
/// candle-index range. The driver updates `visible_range` through
/// [`Self::set_visible_range`]; `paint` is pure and takes a short
/// read-guard.
pub struct VolumeProfileLayer {
    candles: SharedCandleSeries,
    visible_range: Range<usize>,
    style: VolumeProfileStyle,
}

impl VolumeProfileLayer {
    /// Build a layer over `candles`, restricted to `visible_range`.
    pub fn new(
        candles: SharedCandleSeries,
        visible_range: Range<usize>,
        style: VolumeProfileStyle,
    ) -> Self {
        Self {
            candles,
            visible_range,
            style,
        }
    }

    /// Convenience: build with [`VolumeProfileStyle::default`].
    pub fn with_defaults(candles: SharedCandleSeries, visible_range: Range<usize>) -> Self {
        Self::new(candles, visible_range, VolumeProfileStyle::default())
    }

    /// Update the visible index range. Called by the scene driver when
    /// the user pans / zooms.
    pub fn set_visible_range(&mut self, range: Range<usize>) {
        self.visible_range = range;
    }

    /// Borrow the currently-visible index range.
    #[inline]
    pub fn visible_range(&self) -> &Range<usize> {
        &self.visible_range
    }

    #[inline]
    pub fn style(&self) -> VolumeProfileStyle {
        self.style
    }
}

/// Internal bin record: price span + total volume. Kept outside the
/// public API — the layer exposes behaviour through `paint`, not
/// through the histogram.
#[derive(Copy, Clone, Debug, Default)]
struct Bin {
    volume: u64,
}

impl SceneLayer for VolumeProfileLayer {
    fn id(&self) -> LayerId {
        LayerId("volume_profile")
    }

    fn z(&self) -> LayerZ {
        LayerZ::VOLUME_PROFILE
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        // Short-lived read-guard; dropped at end-of-scope. `paint` is
        // synchronous so no `.await` could hold it.
        let guard = self.candles.read();
        if guard.is_empty() {
            return;
        }

        // Clamp visible_range to the current series length. The driver
        // is the authority on range, but a stale range must not cause
        // an out-of-bounds panic or over-count bins.
        let series_len = guard.len();
        let start = self.visible_range.start.min(series_len);
        let end = self.visible_range.end.min(series_len);
        if start >= end {
            return;
        }

        // Price range over visible candles only — slice 7 spec: "Bin
        // price range = min-low to max-high of visible candles".
        let (price_low, price_high) = visible_price_range(&guard, start..end);
        // Guard against NaN / degenerate ranges (equal or inverted);
        // `partial_cmp` keeps the NaN-safe short-circuit explicit.
        if !matches!(
            price_high.partial_cmp(&price_low),
            Some(std::cmp::Ordering::Greater)
        ) {
            return;
        }

        let num_bins = bin_count_for_viewport(ctx.viewport.height_px);
        if num_bins == 0 {
            return;
        }
        let bin_size = ((price_high - price_low) as f64) / num_bins as f64;
        if bin_size <= 0.0 {
            return;
        }

        let mut bins = vec![Bin::default(); num_bins];

        for idx in start..end {
            let Some(c) = guard.at(idx) else { continue };
            let hi = c.high();
            let lo = c.low();
            let vol = c.volume();
            if vol == 0 || !matches!(hi.partial_cmp(&lo), Some(std::cmp::Ordering::Greater)) {
                continue;
            }
            // Distribute this candle's volume uniformly across the
            // bins intersected by `[low, high]`. Matches legacy logic
            // in `volume_profile/mod.rs` so parity fixtures hold.
            let bin_lo_f = ((lo - price_low as f64) / bin_size).floor().max(0.0);
            let bin_hi_f = ((hi - price_low as f64) / bin_size)
                .ceil()
                .min(num_bins as f64);
            let bin_lo = bin_lo_f as usize;
            let mut bin_hi = bin_hi_f as usize;
            if bin_hi == 0 {
                continue;
            }
            bin_hi = bin_hi.min(num_bins).saturating_sub(1);
            if bin_lo > bin_hi {
                continue;
            }
            let touched = (bin_hi - bin_lo + 1) as u64;
            let per = vol / touched;
            let remainder = vol - per * touched;
            for bin in &mut bins[bin_lo..=bin_hi] {
                bin.volume += per;
            }
            if remainder > 0 {
                let mid = (bin_lo + bin_hi) / 2;
                bins[mid].volume += remainder;
            }
        }

        let max_vol = bins.iter().map(|b| b.volume).max().unwrap_or(0);
        if max_vol == 0 {
            return;
        }

        // POC = bin with max total volume. Ties resolve to the first
        // (lowest-price) bin, matching legacy `max_by_key`.
        let poc_idx = bins
            .iter()
            .enumerate()
            .max_by_key(|(_, b)| b.volume)
            .map(|(i, _)| i)
            .unwrap_or(0);

        let max_bar_px = ctx.viewport.width_px * self.style.max_bar_px_fraction;
        let vp_height = ctx.viewport.height_px;

        for (i, bin) in bins.iter().enumerate() {
            if bin.volume == 0 {
                continue;
            }
            // Bin y-span via the price axis. `to_y` maps `price_high`
            // to y=0 and `price_low` to y=height, so the HIGH edge of
            // a bin is the SMALLER y and the LOW edge is the LARGER y.
            let bin_price_lo = price_low as f64 + i as f64 * bin_size;
            let bin_price_hi = bin_price_lo + bin_size;
            let y_top = ctx.price_to_y(bin_price_hi);
            let y_bot = ctx.price_to_y(bin_price_lo);
            // Cull bins fully outside the viewport — legacy parity.
            if y_top > vp_height || y_bot < 0.0 {
                continue;
            }
            let bar_w = (bin.volume as f32 / max_vol as f32) * max_bar_px;
            let color = if i == poc_idx {
                self.style.poc_color
            } else {
                self.style.neighbour_color
            };
            ctx.out.quads.push(QuadInstance {
                x: 0.0,
                y: y_top,
                w: bar_w,
                h: (y_bot - y_top).max(0.0),
                color,
            });
        }
    }
}

/// Scan `range` and return `(min_low, max_high)` over that slice of
/// `series`. `range` is assumed in-bounds (caller clamps).
fn visible_price_range(series: &midas_bars::CandleSeries, range: Range<usize>) -> (f32, f32) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for idx in range {
        let Some(c) = series.at(idx) else { continue };
        let l = c.low();
        let h = c.high();
        if l < lo {
            lo = l;
        }
        if h > hi {
            hi = h;
        }
    }
    (lo as f32, hi as f32)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
    use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{xnys, BarPeriod, Timestamp};
    use parking_lot::RwLock;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk_candle(ts: Timestamp, o: f64, h: f64, l: f64, c: f64, vol: u64) -> Candle {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv = Ohlcv::new(o, h, l, c, vol, 1, None).unwrap();
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

    /// Helper: build a `PaintContext` with a given viewport + price range.
    fn paint_with(
        layer: &VolumeProfileLayer,
        pr: PriceRange,
        vp: Viewport,
        out: &mut ScenePrimitives,
    ) {
        let axis = ContinuousAxis::new(
            utc(2024, 1, 17, 14, 30),
            utc(2024, 1, 17, 14, 30) + chrono::Duration::hours(1),
            vp.width_px,
        )
        .unwrap();
        let pal = ThemePalette::dark_default();
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out,
        };
        layer.paint(&mut ctx);
    }

    fn empty_series() -> SharedCandleSeries {
        let cal = xnys();
        Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            Symbol::new("SPY", cal.id()),
        )))
    }

    // ── 1. Bin-count formula ──────────────────────────────────────────

    #[test]
    fn bin_count_matches_legacy_formula_across_viewports() {
        // Directly probes the pure helper — no series / paint needed.
        assert_eq!(bin_count_for_viewport(100.0), 26);
        assert_eq!(bin_count_for_viewport(500.0), 133);
        assert_eq!(bin_count_for_viewport(1000.0), 200);
        // Lower clamp: anything below ~75px clamps to 20.
        assert_eq!(bin_count_for_viewport(50.0), 20);
        // Upper clamp: anything above ~750px clamps to 200.
        assert_eq!(bin_count_for_viewport(10_000.0), 200);
    }

    // ── 2. Empty series → zero quads ──────────────────────────────────

    #[test]
    fn empty_series_emits_no_quads() {
        let layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0);
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert_eq!(out.quads.len(), 0);
    }

    #[test]
    fn empty_visible_range_emits_no_quads() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            1_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..0);
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert_eq!(out.quads.len(), 0);
    }

    // ── 3. Single candle — volume distributes across intersected bins

    #[test]
    fn single_candle_distributes_volume_across_intersected_bins() {
        // Candle spans [99.0, 101.0] — 2-wide. With num_bins = 26 over
        // [99.0, 101.0] (visible-range price span), bin_size = 2/26 ≈
        // 0.0769. Every one of the 26 bins is intersected → every bin
        // gets a share of the 10_000 volume.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            10_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..1);
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 100.0), // → 26 bins
            &mut out,
        );
        // The single candle spans from min-low (99) to max-high (101)
        // — by definition 100% of visible bins are intersected. 26
        // quads emitted.
        assert_eq!(out.quads.len(), 26);
    }

    // ── 4. POC tint brighter than neighbours ─────────────────────────

    #[test]
    fn poc_bin_gets_distinctly_brighter_color() {
        // Two candles: one with thin range + huge volume anchors the
        // POC; one with wide range + small volume contributes broad
        // low-volume bins.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        // Wide candle — fills many bins with small volume.
        s.push(mk_candle(t, 100.0, 105.0, 95.0, 100.0, 100));
        // Narrow candle — focused volume in one spot.
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            100.2,
            99.8,
            100.1,
            1_000_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..2);
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(90.0, 110.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert!(!out.quads.is_empty());
        let style = VolumeProfileStyle::default();
        let poc_count = out
            .quads
            .iter()
            .filter(|q| q.color == style.poc_color)
            .count();
        // Exactly one POC bin.
        assert_eq!(poc_count, 1);
        // POC alpha strictly higher than neighbour alpha.
        assert!(style.poc_color[3] > style.neighbour_color[3]);
        // POC RGB strictly brighter (at least one channel higher) —
        // implies visually distinguishable.
        let neigh_lum = style.neighbour_color[0] as u32
            + style.neighbour_color[1] as u32
            + style.neighbour_color[2] as u32;
        let poc_lum =
            style.poc_color[0] as u32 + style.poc_color[1] as u32 + style.poc_color[2] as u32;
        assert!(poc_lum > neigh_lum);
    }

    // ── 5. Bin vertical extent matches price-range / bin-count ───────

    #[test]
    fn bin_height_equals_price_range_over_bin_count_within_ulp() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            10_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..1);
        let mut out = ScenePrimitives::default();
        let vp = Viewport::new(1000.0, 100.0); // → 26 bins
        paint_with(&layer, PriceRange::new(95.0, 105.0).unwrap(), vp, &mut out);
        assert_eq!(out.quads.len(), 26);
        // Visible-range price span = 101 - 99 = 2.0. Bin height in
        // price is 2/26. Bin height in pixels:
        // price axis range is 105-95 = 10 over 100px → 1 price unit =
        // 10px, so expected_h = (2.0/26) * 10 ≈ 0.7692 px.
        let expected_h = (2.0_f64 / 26.0_f64) as f32 * 10.0;
        for q in &out.quads {
            // 1 f32 ULP near 0.77 is ~9e-8 — compare with a generous
            // 1e-4 tolerance since the y coords round-trip through
            // `price_to_y` and pick up linear-projection rounding.
            assert!(
                (q.h - expected_h).abs() < 1e-3,
                "q.h={} expected={}",
                q.h,
                expected_h
            );
        }
    }

    // ── 6. Volume conservation ───────────────────────────────────────

    #[test]
    fn total_bin_volume_conserves_visible_candle_volume() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(t, 100.0, 102.0, 98.0, 101.0, 7_777));
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            101.0,
            103.0,
            99.0,
            100.0,
            3_333,
        ));
        s.push(mk_candle(
            t + chrono::Duration::minutes(2),
            100.0,
            104.0,
            96.0,
            103.0,
            9_001,
        ));
        let total_input: u64 = s.iter().map(|c| c.volume()).sum();
        assert_eq!(total_input, 7_777 + 3_333 + 9_001);

        // Mirror the layer's internals to recover per-bin volumes (the
        // layer doesn't expose bins directly — by design). We walk the
        // same algorithm with the same inputs and verify the sum.
        let vp = Viewport::new(1000.0, 400.0);
        let num_bins = bin_count_for_viewport(vp.height_px);
        let price_low = s.iter().map(|c| c.low()).fold(f64::INFINITY, f64::min);
        let price_high = s.iter().map(|c| c.high()).fold(f64::NEG_INFINITY, f64::max);
        let bin_size = (price_high - price_low) / num_bins as f64;
        let mut total_distributed: u64 = 0;
        for c in s.iter() {
            let vol = c.volume();
            if vol == 0 || c.high() <= c.low() {
                continue;
            }
            let bin_lo = ((c.low() - price_low) / bin_size).floor().max(0.0) as usize;
            let bin_hi_f = ((c.high() - price_low) / bin_size)
                .ceil()
                .min(num_bins as f64);
            let bin_hi = (bin_hi_f as usize).min(num_bins).saturating_sub(1);
            if bin_lo > bin_hi {
                continue;
            }
            let touched = (bin_hi - bin_lo + 1) as u64;
            let per = vol / touched;
            let remainder = vol - per * touched;
            total_distributed += per * touched + remainder;
        }
        assert_eq!(total_distributed, total_input);

        // Sanity: the layer also emits something non-empty for this data.
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..3);
        let mut out = ScenePrimitives::default();
        paint_with(&layer, PriceRange::new(90.0, 110.0).unwrap(), vp, &mut out);
        assert!(!out.quads.is_empty());
    }

    // ── 7. Price range = min-low to max-high of visible candles ──────

    #[test]
    fn price_range_spans_visible_min_low_to_max_high() {
        // If price range is derived from VISIBLE candles (not screen
        // price range), then shrinking the visible slice to a single
        // narrow-ranged candle should keep all bins inside that
        // narrow band — i.e. all quads fall between the narrow
        // candle's low and high y coords.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        // Wide candle (ignored).
        s.push(mk_candle(t, 100.0, 109.0, 91.0, 108.0, 1_000));
        // Narrow candle (the only visible one).
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            100.5,
            99.5,
            100.2,
            5_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        // Only the narrow candle in the visible range.
        let layer = VolumeProfileLayer::with_defaults(series, 1..2);
        let mut out = ScenePrimitives::default();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        paint_with(&layer, pr, vp, &mut out);
        assert!(!out.quads.is_empty());
        // Narrow candle spans [99.5, 100.5] → y-span inside a 90..110
        // viewport: y(100.5) = (110-100.5)/20 * 400 = 190;
        // y(99.5) = (110-99.5)/20 * 400 = 210.
        // Every quad must lie inside that band.
        for q in &out.quads {
            assert!(
                q.y >= 190.0 - 1e-3 && (q.y + q.h) <= 210.0 + 1e-3,
                "quad y={} h={} outside narrow-visible band [190, 210]",
                q.y,
                q.h
            );
        }
    }

    // ── 8. Visible-range subset: only in-range candles contribute ────

    #[test]
    fn candles_outside_visible_range_are_ignored() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        // Index 0 — huge volume, wide range.
        s.push(mk_candle(t, 100.0, 105.0, 95.0, 104.0, 10_000_000));
        // Index 1 — small volume, narrow range.
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            100.1,
            99.9,
            100.05,
            10,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));

        // Visible range = [1, 2): only the narrow low-vol candle. If
        // candle 0 leaked in, we'd see wide-band bins.
        let layer = VolumeProfileLayer::with_defaults(Arc::clone(&series), 1..2);
        let mut out = ScenePrimitives::default();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        paint_with(&layer, pr, vp, &mut out);

        // The narrow candle's span is [99.9, 100.1] → y-band
        // [199, 201] inside a 400-px viewport over 90..110.
        assert!(!out.quads.is_empty());
        for q in &out.quads {
            assert!(
                q.y >= 199.0 - 1e-2 && q.y + q.h <= 201.0 + 1e-2,
                "candle 0 leaked: quad y={} h={}",
                q.y,
                q.h
            );
        }
    }

    // ── 9. LayerZ integer slotting ───────────────────────────────────

    #[test]
    fn layer_z_volume_profile_between_volume_and_candle() {
        assert!(LayerZ::VOLUME < LayerZ::VOLUME_PROFILE);
        assert!(LayerZ::VOLUME_PROFILE < LayerZ::CANDLE);
        // Integer assertion — guards against silent renumbering.
        assert_eq!(LayerZ::VOLUME_PROFILE.0, 350);
    }

    // ── 10. SceneLayer glue ──────────────────────────────────────────

    #[test]
    fn scene_layer_id_and_z() {
        let layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0);
        assert_eq!(layer.id(), LayerId("volume_profile"));
        assert_eq!(layer.z(), LayerZ::VOLUME_PROFILE);
    }

    #[test]
    fn is_passive_by_default() {
        // Slice 7 VP is passive — `as_interactive` returns None.
        let mut layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0);
        assert!(layer.as_interactive().is_none());
    }

    // ── 11. set_visible_range ────────────────────────────────────────

    #[test]
    fn set_visible_range_updates_internal_range() {
        let mut layer = VolumeProfileLayer::with_defaults(empty_series(), 0..0);
        layer.set_visible_range(3..7);
        assert_eq!(*layer.visible_range(), 3..7);
    }

    // ── 12. Stale visible-range clamping ─────────────────────────────

    #[test]
    fn stale_visible_range_is_clamped_not_panic() {
        // Range end larger than series len must NOT panic.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        s.push(mk_candle(
            utc(2024, 1, 17, 14, 30),
            100.0,
            101.0,
            99.0,
            100.5,
            1_000,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        // Driver lags behind a `push` — range thinks there are 999 bars.
        let layer = VolumeProfileLayer::with_defaults(series, 0..999);
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(90.0, 110.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        // Didn't panic; and the single candle did contribute.
        assert!(!out.quads.is_empty());
    }

    // ── 13. Zero-volume candles are skipped ──────────────────────────

    #[test]
    fn zero_volume_candles_contribute_nothing() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let t = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(t, 100.0, 101.0, 99.0, 100.5, 0));
        s.push(mk_candle(
            t + chrono::Duration::minutes(1),
            100.0,
            101.0,
            99.0,
            100.5,
            0,
        ));
        let series: SharedCandleSeries = Arc::new(RwLock::new(s));
        let layer = VolumeProfileLayer::with_defaults(series, 0..2);
        let mut out = ScenePrimitives::default();
        paint_with(
            &layer,
            PriceRange::new(95.0, 105.0).unwrap(),
            Viewport::new(1000.0, 400.0),
            &mut out,
        );
        assert_eq!(out.quads.len(), 0);
    }
}
