//! OHLC snap algorithm — ports the legacy `level_tool::snap_to_ohlc` to
//! the new sans-IO axis model.
//!
//! The algorithm (plan slice 4, corrected C4 + C5):
//!
//! 1. Find the candle nearest `cursor_x_px` by mapping the cursor x back
//!    through the [`TimeAxis`].
//! 2. Examine that candle plus its ±1 neighbours (≤ 3 total).
//! 3. For each candle, compute the y-distance from `cursor_price` to
//!    each of `[open, high, low, close]` (in that iteration order) via
//!    [`PriceAxis::to_y`]. Strict `<` comparison so ties resolve to
//!    the first encountered value — priority `Open < High < Low < Close`.
//! 4. If the best y-distance is ≤ the adaptive threshold, return the
//!    snapped price; else return `cursor_price` unchanged.
//!
//! The threshold is `candle_width_px.clamp(MIN, MAX)` so dense charts
//! snap in a narrow band and sparse charts snap over a wider one.
//! Constants match the plan's corrected values:
//! `SNAP_THRESHOLD_MIN_PX = 3.0`, `SNAP_THRESHOLD_MAX_PX = 12.0`.
//!
//! The function is pure: no allocation, no logging, no side effects.

use midas_axis::{PriceAxis, TimeAxis};

/// Minimum pixel distance (Y-axis) for OHLC snap.
///
/// Plan C5 pinned this at 3.0 px for the new-stack snap. The legacy
/// `level_tool::mod.rs` uses 15.0; the new value gives a tighter snap
/// band on dense charts. Validated by the adaptive-threshold tests in
/// `tests/level_tool_flow.rs`.
pub const SNAP_THRESHOLD_MIN_PX: f32 = 3.0;

/// Maximum pixel distance (Y-axis) for OHLC snap.
///
/// Plan C5 pinned this at 12.0 px (legacy was 40.0).
pub const SNAP_THRESHOLD_MAX_PX: f32 = 12.0;

/// Minimal OHLC candle record — avoids pulling `midas-bars::CandleRef`
/// (which borrows a whole series) into the snap surface. Widget code
/// projects every visible candle into one of these before calling
/// [`snap_to_ohlc`].
///
/// The lifetime parameter is kept for forward-compatibility with a
/// future borrow-based variant (e.g. `&midas_bars::CandleRef<'a>`); for
/// now the struct is `Copy` so call sites can build transient views.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CandleRef<'a> {
    /// The bar's open timestamp (nanoseconds since UNIX epoch). Kept as
    /// a raw i64 so the snap math never allocates a `chrono::DateTime`.
    pub ts_open_ns: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Borrow marker so a future variant can carry `&'a CandleSeries`
    /// without a breaking change. Zero-sized in the current impl.
    pub _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> CandleRef<'a> {
    /// Convenience: build a view from raw OHLC + timestamp nanoseconds.
    /// Tests use this; widget code maps from `midas_bars::CandleRef`.
    pub fn new(ts_open_ns: i64, open: f64, high: f64, low: f64, close: f64) -> Self {
        Self {
            ts_open_ns,
            open,
            high,
            low,
            close,
            _marker: std::marker::PhantomData,
        }
    }
}

/// Snap a cursor price to the nearest OHLC of one of the candles near
/// `cursor_x_px`. See module docs for the full algorithm.
///
/// - `cursor_price`: the raw price under the cursor (`axis.from_y`).
/// - `cursor_x_px`: the cursor x-pixel.
/// - `candles`: the full candle window. The function picks the
///   nearest-by-x entry + its ±1 neighbours.
/// - `price_axis`: projects prices to y-pixels for the distance compare.
/// - `time_axis`: projects the cursor x back to a timestamp so the
///   nearest-candle search is a binary-scan by `ts_open`.
/// - `candle_width_px`: caller-computed visible candle width. Drives
///   the adaptive threshold. Clamped into `[MIN, MAX]`.
///
/// Returns the snapped price if within threshold; else returns
/// `cursor_price` unchanged.
pub fn snap_to_ohlc(
    cursor_price: f64,
    cursor_x_px: f32,
    candles: &[CandleRef<'_>],
    price_axis: &dyn PriceAxis,
    time_axis: &dyn TimeAxis,
    candle_width_px: f32,
) -> f64 {
    if candles.is_empty() {
        return cursor_price;
    }

    let cursor_y = price_axis.to_y(cursor_price);
    let len = candles.len();

    // 1. Nearest candle index by x. Route the cursor x through the
    //    time axis to get a timestamp, then binary-search by `ts_open_ns`.
    //    If the x is inside a compressed gap (`from_x` returns None),
    //    fall back to the mid-slot.
    let nearest_idx = match time_axis.from_x(cursor_x_px) {
        Some(ts) => nearest_by_ts(candles, ts.timestamp_nanos_opt().unwrap_or(0)),
        None => len / 2,
    };

    // 2. ±1 window.
    let start = nearest_idx.saturating_sub(1);
    let end = (nearest_idx + 2).min(len);

    // 3. Adaptive threshold.
    let snap_threshold_px = candle_width_px.clamp(SNAP_THRESHOLD_MIN_PX, SNAP_THRESHOLD_MAX_PX);

    // 4. For each candle in the window, iterate [open, high, low, close]
    //    with strict `<` so ties land on the earliest in the order —
    //    Open < High < Low < Close.
    let mut best_price = cursor_price;
    let mut best_dist = f32::MAX;

    for c in candles.iter().take(end).skip(start) {
        for &p in &[c.open, c.high, c.low, c.close] {
            let py = price_axis.to_y(p);
            let dist = (py - cursor_y).abs();
            if dist < best_dist {
                best_dist = dist;
                best_price = p;
            }
        }
    }

    if best_dist <= snap_threshold_px {
        best_price
    } else {
        cursor_price
    }
}

/// Return the index of the candle whose `ts_open_ns` is closest to `ts_ns`.
/// Linear scan — the window is small enough (≤ 5K bars) that a binary
/// search's branch-predictor wins are not load-bearing here, and a flat
/// scan is easier to audit against the legacy implementation.
fn nearest_by_ts(candles: &[CandleRef<'_>], ts_ns: i64) -> usize {
    let mut best_idx = 0usize;
    let mut best_diff = i64::MAX;
    for (i, c) in candles.iter().enumerate() {
        let diff = (c.ts_open_ns - ts_ns).abs();
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, LinearPriceAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn ns(t: Timestamp) -> i64 {
        t.timestamp_nanos_opt().unwrap()
    }

    /// Build a 3-candle harness at ts = 10:00, 10:01, 10:02.
    fn three_candles() -> (
        Vec<CandleRef<'static>>,
        ContinuousAxis,
        LinearPriceAxis,
        f32,
    ) {
        let t0 = ts(2024, 1, 1, 10, 0);
        let t1 = ts(2024, 1, 1, 10, 1);
        let t2 = ts(2024, 1, 1, 10, 2);

        let candles = vec![
            CandleRef::new(ns(t0), 100.0, 101.0, 99.0, 100.5),
            CandleRef::new(ns(t1), 100.5, 102.0, 100.0, 101.5),
            CandleRef::new(ns(t2), 101.5, 103.0, 101.0, 102.5),
        ];

        // Price range 90..110, viewport 400 px tall → 1 price unit = 20 px.
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        // Time axis: 10:00 → 10:03 across 1000 px (3 candles × ~333 px each).
        let start = ts(2024, 1, 1, 10, 0);
        let end = ts(2024, 1, 1, 10, 3);
        let taxis = ContinuousAxis::new(start, end, vp.width_px).unwrap();

        // candle_width_px = viewport_width / visible_candles = 1000 / 3 ≈ 333 px.
        // For the snap tests we want the threshold clamped to MAX (12 px).
        let candle_width_px = 333.0_f32;
        (candles, taxis, paxis, candle_width_px)
    }

    #[test]
    fn snap_empty_candles_returns_cursor_price() {
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let paxis = LinearPriceAxis::new(pr, 400.0);
        let taxis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0), ts(2024, 1, 2, 0, 0), 1000.0).unwrap();
        let snapped = snap_to_ohlc(100.42, 500.0, &[], &paxis, &taxis, 10.0);
        assert_eq!(snapped, 100.42);
    }

    #[test]
    fn snap_near_open_resolves_to_open_over_close_on_tie() {
        // Construct a scenario where open == close exactly so the
        // iteration order `[open, high, low, close]` with strict `<`
        // resolves the tie to `open`.
        let t0 = ts(2024, 1, 1, 10, 0);
        let candles = vec![CandleRef::new(ns(t0), 100.0, 101.0, 99.0, 100.0)];
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let paxis = LinearPriceAxis::new(pr, 400.0);
        // Single-candle axis: 10:00 → 10:01 across 1000 px.
        let start = t0;
        let end = ts(2024, 1, 1, 10, 1);
        let taxis = ContinuousAxis::new(start, end, 1000.0).unwrap();

        // Cursor sits exactly on 100.0 — open and close both match.
        let snapped = snap_to_ohlc(100.0, 500.0, &candles, &paxis, &taxis, 10.0);
        assert_eq!(snapped, 100.0);
        // And for a near-miss that's equidistant from 100.0 and (say) 99.0:
        // 99.5 is 0.5 from 100.0 (open) and 0.5 from 99.0 (low); iteration
        // order resolves to `open`.
        let snapped = snap_to_ohlc(99.5, 500.0, &candles, &paxis, &taxis, 10.0);
        // With threshold clamped to MAX = 12 px, both candidates are within
        // range (0.5 price = 10 px). The one first in iteration order wins:
        // [open=100.0, high=101.0, low=99.0, close=100.0]. 100.0 has
        // dist 10 px; 99.0 also has dist 10 px; strict `<` keeps `open`.
        assert_eq!(snapped, 100.0);
    }

    #[test]
    fn snap_adaptive_threshold_clamps_at_min_3px() {
        // When `candle_width_px` is below MIN (3.0), the threshold
        // clamps UP to 3 px. A cursor off by 4 px from an OHLC value
        // should NOT snap because 4 > 3. A cursor off by 2 px should
        // snap.
        let (candles, taxis, paxis, _cw) = three_candles();
        // Use a deliberately tiny candle_width_px (1.0) → threshold = 3 px.
        let tight_cw = 1.0_f32;

        // Price 100.5 sits at y = (110-100.5)/(110-90) * 400 = 9.5/20 * 400 = 190 px.
        // Candle[1].open = 100.5 → same y = 190.
        // Place cursor 2 px above → y = 188 → price = 110 - 188/400 * 20 = 110 - 9.4 = 100.6.
        let snapped = snap_to_ohlc(100.6, 500.0, &candles, &paxis, &taxis, tight_cw);
        assert_eq!(snapped, 100.5, "2 px off snaps under MIN threshold");

        // 4 px above → y = 186 → price = 110 - 186/400 * 20 = 110 - 9.3 = 100.7.
        let snapped = snap_to_ohlc(100.7, 500.0, &candles, &paxis, &taxis, tight_cw);
        assert_eq!(snapped, 100.7, "4 px off does NOT snap under MIN threshold");
    }

    #[test]
    fn snap_adaptive_threshold_clamps_at_max_12px() {
        // When `candle_width_px` is above MAX (12.0), the threshold
        // clamps DOWN to 12 px. A cursor just over 12 px off should
        // NOT snap; a cursor just under 12 px off should snap.
        //
        // Place an isolated OHLC value so the nearest-OHLC test is
        // unambiguous: build a single candle at ts=10:00 with a value
        // at price 50.0 (far from any other scene value). Price range
        // 0..200, height 400 → 1 price unit = 2 px.
        let t0 = ts(2024, 1, 1, 10, 0);
        let candles = vec![CandleRef::new(ns(t0), 50.0, 50.0, 50.0, 50.0)];
        let pr = PriceRange::new(0.0, 200.0).unwrap();
        let paxis = LinearPriceAxis::new(pr, 400.0);
        let start = t0;
        let end = ts(2024, 1, 1, 10, 1);
        let taxis = ContinuousAxis::new(start, end, 1000.0).unwrap();

        let wide_cw = 10_000.0_f32; // clamps to MAX = 12.

        // Cursor at 55.5 → y = 289; candle y = 300 → 11 px off. SNAP.
        let snapped = snap_to_ohlc(55.5, 500.0, &candles, &paxis, &taxis, wide_cw);
        assert_eq!(snapped, 50.0, "11 px off snaps under MAX threshold");

        // Cursor at 56.5 → y = 287; 13 px off. NO snap.
        let snapped = snap_to_ohlc(56.5, 500.0, &candles, &paxis, &taxis, wide_cw);
        assert_eq!(snapped, 56.5, "13 px off does NOT snap under MAX threshold");
    }

    #[test]
    fn snap_window_is_plus_minus_one_candle() {
        // Verify only the nearest candle's ±1 neighbours are examined.
        // We put a matching OHLC value at candle[0] (far from cursor_x),
        // and verify the cursor near candle[2] snaps to candle[2]'s
        // OHLC, not to candle[0]'s, because [0] is outside the window.
        let t0 = ts(2024, 1, 1, 10, 0);
        let t1 = ts(2024, 1, 1, 10, 1);
        let t2 = ts(2024, 1, 1, 10, 2);
        let t3 = ts(2024, 1, 1, 10, 3);
        let t4 = ts(2024, 1, 1, 10, 4);

        // Five candles. Candle [0] has a price that would be even closer
        // to the cursor price — but it's out of window when the cursor
        // is near candle [4].
        let candles = vec![
            CandleRef::new(ns(t0), 100.2, 100.3, 100.1, 100.2), // Too close in price
            CandleRef::new(ns(t1), 50.0, 51.0, 49.0, 50.5),
            CandleRef::new(ns(t2), 60.0, 61.0, 59.0, 60.5),
            CandleRef::new(ns(t3), 70.0, 71.0, 69.0, 70.5),
            CandleRef::new(ns(t4), 80.0, 81.0, 79.0, 80.5),
        ];
        let pr = PriceRange::new(0.0, 200.0).unwrap();
        let paxis = LinearPriceAxis::new(pr, 400.0);
        // 5 candles across 1000 px → 200 px per candle.
        let start = t0;
        let end = ts(2024, 1, 1, 10, 5);
        let taxis = ContinuousAxis::new(start, end, 1000.0).unwrap();
        let cw = 200.0_f32; // clamped to MAX = 12.

        // Cursor x = 900 px → nearest candle is [4] (ts = 10:04). Price
        // 80.1 — close to candle[4].open (80.0, 40 px off? let's check:
        // pr=200, vp=400 → 1 price unit = 2 px. 0.1 price = 0.2 px.
        // WITHIN threshold → snap to 80.0).
        let snapped = snap_to_ohlc(80.1, 900.0, &candles, &paxis, &taxis, cw);
        assert_eq!(snapped, 80.0, "snap to candle[4].open");

        // Cursor x = 900 px, cursor price 100.2 — candle[0] has this
        // value but is OUTSIDE the ±1 window around candle[4]. The
        // closest OHLC in [3..=4] is candle[4].high = 81.0; distance
        // in y = (100.2 - 81) * 2 = 38.4 px, well beyond threshold.
        // Expect: NO snap.
        let snapped = snap_to_ohlc(100.2, 900.0, &candles, &paxis, &taxis, cw);
        assert_eq!(
            snapped, 100.2,
            "candle[0] is outside window; no in-window OHLC close enough"
        );
    }

    #[test]
    fn snap_ties_resolve_open_lt_high_lt_low_lt_close() {
        // Construct a single candle with all-four-values equal → every
        // OHLC has the same y-distance. Iteration order puts `open`
        // first with strict `<`, so the tie resolves to `open`.
        let t0 = ts(2024, 1, 1, 10, 0);
        let candles = vec![CandleRef::new(ns(t0), 100.0, 100.0, 100.0, 100.0)];
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let paxis = LinearPriceAxis::new(pr, 400.0);
        let start = t0;
        let end = ts(2024, 1, 1, 10, 1);
        let taxis = ContinuousAxis::new(start, end, 1000.0).unwrap();
        let cw = 333.0_f32;
        // Cursor exactly on 100.0 → best_dist = 0 for all; first in
        // iteration order (open) wins. But since they're all equal,
        // this test mainly pins that the resolver prefers the earliest.
        let snapped = snap_to_ohlc(100.0, 500.0, &candles, &paxis, &taxis, cw);
        assert_eq!(snapped, 100.0);

        // Asymmetric tie: open = high = 100.0, low = close = 101.0.
        // Cursor at 100.5 is 10 px from both. iteration order
        // [open=100, high=100, low=101, close=101]: first 100 at dist 10;
        // second 100 at dist 10 → strict `<` keeps `open`. Then 101 at
        // dist 10 also NOT less than current best → stays `open`.
        let candles = vec![CandleRef::new(ns(t0), 100.0, 100.0, 101.0, 101.0)];
        let snapped = snap_to_ohlc(100.5, 500.0, &candles, &paxis, &taxis, cw);
        assert_eq!(snapped, 100.0, "tie resolves to open (first in order)");
    }
}
