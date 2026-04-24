//! Per-chart mutable interaction state.
//!
//! Per R2-NM-7 the old `Camera2D` is deleted; projection state lives
//! on the axis + price range + viewport. What REMAINS as mutable,
//! per-chart state is the interaction bookkeeping — hover target,
//! drag session, crosshair pixel.
//!
//! This type is moved into `midas-scene` now so downstream slices
//! (S8 `SessionChart`, S9 `chart-input`) can depend on it without
//! pulling any iced or wgpu machinery. Paint is pure; event handlers
//! mutate this state and dirty the projection.
//!
//! ## Slice 2b additions
//!
//! Pan / zoom / auto-scale helpers land on this module (sans-IO,
//! sitting next to [`InteractionState`] so the widget layer can pick
//! them up without a new crate). See [`pan_time_window`],
//! [`zoom_time_window_at`], [`zoom_price_range_at`], and
//! [`auto_scale_price`].
//!
//! `paint_pending` sits on [`InteractionState`] to support slice 2c's
//! tick-cadence coalescing: `update_last_price` sets the flag; the
//! widget's render loop clears it after building one frame. A tick
//! arriving while another is pending is a no-op on the render-cost
//! side — saves O(20 panel × 100 Hz) redundant redraws.

use std::sync::atomic::{AtomicBool, Ordering};

use midas_calendar::Timestamp;

/// What the pointer is currently over, if anything. Populated by
/// hit-test routines; consumed by decorator visibility rules and by
/// hover highlights in each layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HoverTarget {
    /// The Nth candle in the active [`CandleSeries`](midas_bars::CandleSeries).
    Candle(usize),
    /// A leg of the named order bracket.
    Bracket { id: u64, leg: BracketLeg },
    /// A named price-line annotation.
    PriceLine(u64),
    /// A named price level annotation.
    Level(u64),
}

/// Which leg of a three-leg order bracket the pointer sits on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BracketLeg {
    Entry,
    TakeProfit,
    StopLoss,
}

/// An in-flight drag. `start_px` is captured at `mouse_down`;
/// `current_px` updates on `mouse_move`; releasing commits or
/// cancels the drag and the session is cleared.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DragSession {
    pub target: HoverTarget,
    pub start_px: (f32, f32),
    pub current_px: (f32, f32),
}

/// Mutable per-chart interaction state. All fields are directly `pub`
/// so the input-layer slice (S9) can mutate in-place without a
/// proliferation of setters. The type is deliberately simple — this
/// is the minimum surface every chart needs; richer state (e.g.
/// bracket-placement 3-click state machine) lives in the specific
/// tool, not here.
///
/// ## Slice 2c addition
///
/// `paint_pending` is an [`AtomicBool`] flag the tick-cadence fan-out
/// sets when a [`midas_bars::CandleSeries::update_last_price`] call
/// bumps the last candle's close — the render loop observes + clears
/// the flag per frame. Wrapped in [`AtomicBool`] rather than `bool`
/// so the handler can flip it from any thread without going through
/// the state's write lock.
#[derive(Debug, Default)]
pub struct InteractionState {
    pub hover: Option<HoverTarget>,
    pub drag: Option<DragSession>,
    pub crosshair_px: Option<(f32, f32)>,
    /// Slice 2c: true iff a tick has invalidated the chart since the
    /// last paint. Cheap to flip from a quote-batch handler, cheap to
    /// observe from the iced render loop. Not a replacement for the
    /// `CandleSeries::version()` watch channel — that drives PAINT
    /// scheduling. This flag coalesces REPAINT-requests within a
    /// frame.
    pub paint_pending: AtomicBool,
}

impl Clone for InteractionState {
    fn clone(&self) -> Self {
        Self {
            hover: self.hover,
            drag: self.drag,
            crosshair_px: self.crosshair_px,
            paint_pending: AtomicBool::new(self.paint_pending.load(Ordering::Relaxed)),
        }
    }
}

impl InteractionState {
    /// Build a fresh interaction state — hover, drag, and crosshair
    /// all `None`; `paint_pending` false.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the state as needing a repaint. Cheap, thread-safe.
    #[inline]
    pub fn mark_paint_pending(&self) {
        self.paint_pending.store(true, Ordering::Relaxed);
    }

    /// Consume + clear the paint-pending flag. Returns `true` iff a
    /// repaint had been requested since the last clear.
    #[inline]
    pub fn take_paint_pending(&self) -> bool {
        self.paint_pending.swap(false, Ordering::Relaxed)
    }
}

// ── Pan / zoom / auto-scale helpers (slice 2b) ────────────────────────
//
// These are FREE FUNCTIONS rather than methods on `InteractionState`
// because they operate on the axis + price-range projection — state
// that lives on the widget, not on `InteractionState`. Co-locating
// them here keeps the sans-IO surface in one module for downstream
// slices (session_chart::widget, chart-input).

/// Minimum number of candles that must remain visible after a zoom
/// operation. Prevents "zoom into nothing".
pub const MIN_VISIBLE_CANDLES: usize = 10;

/// Maximum supported zoom-out: 10 years. Clamps user-driven zoom so a
/// mis-scroll doesn't stretch the axis to a degenerate nanosecond-
/// span.
pub const MAX_ZOOM_OUT_YEARS: i64 = 10;

/// Pan the visible time window by `dx_px` pixels to the right.
/// Negative values pan left.
///
/// `axis_width_px` is the width of the chart drawing area.
///
/// Returns the new `(start, end)` wall-clock range with the same span
/// but shifted by `(dx_px / axis_width_px) * span`. Clamping is the
/// caller's responsibility — this helper does not know the data's
/// real time bounds.
pub fn pan_time_window(
    window: (Timestamp, Timestamp),
    dx_px: f32,
    axis_width_px: f32,
) -> (Timestamp, Timestamp) {
    if !dx_px.is_finite() || !axis_width_px.is_finite() || axis_width_px <= 0.0 {
        return window;
    }
    let (start, end) = window;
    let span_ns = (end - start).num_nanoseconds().unwrap_or(0);
    if span_ns <= 0 {
        return window;
    }
    let frac = dx_px as f64 / axis_width_px as f64;
    let shift_ns = (frac * span_ns as f64) as i64;
    let shifted_start = start
        .checked_add_signed(chrono::Duration::nanoseconds(shift_ns))
        .unwrap_or(start);
    let shifted_end = end
        .checked_add_signed(chrono::Duration::nanoseconds(shift_ns))
        .unwrap_or(end);
    tracing::debug!(
        target: "midas_scene::interaction::pan_time_window",
        dx_px,
        axis_width_px,
        shift_ns,
        "pan window",
    );
    (shifted_start, shifted_end)
}

/// Zoom the visible time window around an anchor x-pixel — the time
/// under that pixel stays under the same pixel after the zoom.
///
/// `factor > 1.0` zooms OUT (widens the visible span); `< 1.0` zooms
/// IN (narrows).
///
/// Enforces [`MIN_VISIBLE_CANDLES`] × `candle_width_ns` as the minimum
/// new span and [`MAX_ZOOM_OUT_YEARS`] as the maximum, so callers
/// can't accidentally produce a degenerate axis.
pub fn zoom_time_window_at(
    window: (Timestamp, Timestamp),
    anchor_x_px: f32,
    axis_width_px: f32,
    factor: f32,
    candle_width_ns: i64,
) -> (Timestamp, Timestamp) {
    if !factor.is_finite() || factor <= 0.0 {
        return window;
    }
    if !axis_width_px.is_finite() || axis_width_px <= 0.0 {
        return window;
    }
    if !anchor_x_px.is_finite() {
        return window;
    }
    let (start, end) = window;
    let span_ns = (end - start).num_nanoseconds().unwrap_or(0);
    if span_ns <= 0 {
        return window;
    }
    let anchor_frac = (anchor_x_px as f64 / axis_width_px as f64).clamp(0.0, 1.0);
    let anchor_ns =
        start.timestamp_nanos_opt().unwrap_or(0) + (anchor_frac * span_ns as f64) as i64;

    let mut new_span_ns = (span_ns as f64 * factor as f64) as i64;

    // Clamp to min 10 candles and max 10 years.
    let min_span = (MIN_VISIBLE_CANDLES as i64).saturating_mul(candle_width_ns.max(1));
    let max_span = chrono::Duration::days(365 * MAX_ZOOM_OUT_YEARS)
        .num_nanoseconds()
        .unwrap_or(i64::MAX);
    new_span_ns = new_span_ns.clamp(min_span, max_span);

    // Re-anchor so the pixel `anchor_x_px` still maps to `anchor_ns`.
    let new_start_ns = anchor_ns - (anchor_frac * new_span_ns as f64) as i64;
    let new_end_ns = new_start_ns + new_span_ns;
    let new_start = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(new_start_ns);
    let new_end = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(new_end_ns);
    tracing::debug!(
        target: "midas_scene::interaction::zoom_time_window_at",
        anchor_x_px,
        factor,
        old_span_ns = span_ns,
        new_span_ns,
        "zoom window",
    );
    (new_start, new_end)
}

/// Zoom the price range around a y-pixel anchor — the price under
/// that y stays under the same y after the zoom.
///
/// `factor > 1.0` zooms OUT (widens the price range); `< 1.0` zooms
/// IN. Returns the new `PriceRange` or `None` if the computed range
/// is degenerate (e.g. factor = 0 collapsed to a single price).
pub fn zoom_price_range_at(
    range: midas_axis::PriceRange,
    anchor_y_px: f32,
    axis_height_px: f32,
    factor: f32,
) -> Option<midas_axis::PriceRange> {
    if !factor.is_finite() || factor <= 0.0 {
        return None;
    }
    if !axis_height_px.is_finite() || axis_height_px <= 0.0 {
        return None;
    }
    if !anchor_y_px.is_finite() {
        return None;
    }
    let span = range.span();
    if span <= 0.0 {
        return None;
    }
    let anchor_frac = (anchor_y_px as f64 / axis_height_px as f64).clamp(0.0, 1.0);
    let anchor_price = range.high() - anchor_frac * span;
    let new_span = span * factor as f64;
    let new_high = anchor_price + anchor_frac * new_span;
    let new_low = new_high - new_span;
    tracing::debug!(
        target: "midas_scene::interaction::zoom_price_range_at",
        anchor_y_px,
        anchor_price,
        factor,
        old_span = span,
        new_span,
        "zoom price",
    );
    midas_axis::PriceRange::new(new_low, new_high).ok()
}

/// Auto-scale the price axis to fit the high/low of the visible
/// candles with a 5% vertical pad.
///
/// - Empty slice / out-of-range indices → `None` (caller retains
///   existing range).
/// - `high == low` on a degenerate series — pad by the greater of
///   `0.01 * price` and `0.01` to avoid a zero-span range.
/// - Non-finite OHLC → rows are skipped; if the filtered run is
///   empty, `None`.
pub fn auto_scale_price(
    series: &midas_bars::CandleSeries,
    visible_range: std::ops::Range<usize>,
) -> Option<midas_axis::PriceRange> {
    if visible_range.is_empty() {
        return None;
    }
    let mut hi = f64::MIN;
    let mut lo = f64::MAX;
    let end = visible_range.end.min(series.len());
    let start = visible_range.start.min(end);
    for idx in start..end {
        let Some(row) = series.at(idx) else { break };
        let h = row.high();
        let l = row.low();
        if !h.is_finite() || !l.is_finite() {
            continue;
        }
        if h > hi {
            hi = h;
        }
        if l < lo {
            lo = l;
        }
    }
    if hi == f64::MIN || lo == f64::MAX {
        return None;
    }
    let (new_lo, new_hi) = if (hi - lo).abs() < f64::EPSILON {
        // Degenerate: high == low. Pad by 1% of price (min 0.01).
        let pad = (0.01 * hi.abs()).max(0.01);
        (lo - pad, hi + pad)
    } else {
        let span = hi - lo;
        let pad = span * 0.05;
        (lo - pad, hi + pad)
    };
    tracing::debug!(
        target: "midas_scene::interaction::auto_scale_price",
        raw_low = lo,
        raw_high = hi,
        padded_low = new_lo,
        padded_high = new_hi,
        "auto-scale price",
    );
    midas_axis::PriceRange::new(new_lo, new_hi).ok()
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};
    use midas_axis::PriceRange;
    use midas_bars::{BarPeriod, Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::xnys;

    use super::*;

    #[test]
    fn default_is_all_none() {
        let s = InteractionState::new();
        assert!(s.hover.is_none());
        assert!(s.drag.is_none());
        assert!(s.crosshair_px.is_none());
        assert!(!s.take_paint_pending());
    }

    #[test]
    fn hover_target_equality() {
        let a = HoverTarget::Bracket {
            id: 7,
            leg: BracketLeg::TakeProfit,
        };
        let b = HoverTarget::Bracket {
            id: 7,
            leg: BracketLeg::TakeProfit,
        };
        let c = HoverTarget::Bracket {
            id: 7,
            leg: BracketLeg::StopLoss,
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn drag_session_is_copy() {
        fn takes_copy<T: Copy>(_: T) {}
        takes_copy(DragSession {
            target: HoverTarget::Candle(0),
            start_px: (0.0, 0.0),
            current_px: (0.0, 0.0),
        });
    }

    #[test]
    fn paint_pending_round_trips() {
        let s = InteractionState::new();
        assert!(!s.take_paint_pending());
        s.mark_paint_pending();
        assert!(s.take_paint_pending());
        // Second take returns false — the flag is cleared.
        assert!(!s.take_paint_pending());
    }

    #[test]
    fn paint_pending_coalesces_bursts() {
        // 100 marks + 1 take == one repaint.
        let s = InteractionState::new();
        for _ in 0..100 {
            s.mark_paint_pending();
        }
        assert!(s.take_paint_pending());
        assert!(!s.take_paint_pending());
    }

    #[test]
    fn clone_preserves_paint_pending_flag() {
        let s = InteractionState::new();
        s.mark_paint_pending();
        let c = s.clone();
        // Cloning reads via `Ordering::Relaxed`; both should observe
        // the flag set.
        assert!(s.take_paint_pending());
        assert!(c.take_paint_pending());
    }

    // ── Pan helpers ─────────────────────────────────────────────────

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn pan_right_shifts_window_forward() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = pan_time_window((start, end), 100.0, 1000.0);
        // 10% of 1-day → 2.4 h shift.
        assert!(ns > start);
        assert!(ne > end);
        assert_eq!(ne - ns, end - start, "span preserved");
    }

    #[test]
    fn pan_left_shifts_window_backward() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = pan_time_window((start, end), -100.0, 1000.0);
        assert!(ns < start);
        assert!(ne < end);
    }

    #[test]
    fn pan_is_monotonic() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let w0 = (start, end);
        let w1 = pan_time_window(w0, 50.0, 1000.0);
        let w2 = pan_time_window(w1, 50.0, 1000.0);
        let w3 = pan_time_window(w0, 100.0, 1000.0);
        // Two 50 px pans ≈ one 100 px pan.
        assert_eq!(w2.0, w3.0);
        assert_eq!(w2.1, w3.1);
    }

    #[test]
    fn pan_zero_dx_is_noop() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = pan_time_window((start, end), 0.0, 1000.0);
        assert_eq!(ns, start);
        assert_eq!(ne, end);
    }

    #[test]
    fn pan_zero_width_is_noop() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = pan_time_window((start, end), 100.0, 0.0);
        assert_eq!(ns, start);
        assert_eq!(ne, end);
    }

    #[test]
    fn pan_nan_dx_is_noop() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = pan_time_window((start, end), f32::NAN, 1000.0);
        assert_eq!(ns, start);
        assert_eq!(ne, end);
    }

    // ── Zoom helpers ────────────────────────────────────────────────

    #[test]
    fn zoom_preserves_anchor_point() {
        // Zoom in by 0.5 anchored at the mid-pixel; the time under
        // the mid-pixel before must equal the time under the mid-
        // pixel after.
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let width = 1000.0;
        let anchor_px = 500.0;
        let before_anchor_ns = start.timestamp_nanos_opt().unwrap()
            + ((end - start).num_nanoseconds().unwrap() as f64 * 0.5) as i64;
        let candle_width_ns = Duration::minutes(1).num_nanoseconds().unwrap();
        let (ns, ne) = zoom_time_window_at((start, end), anchor_px, width, 0.5, candle_width_ns);
        let after_anchor_ns = ns.timestamp_nanos_opt().unwrap()
            + ((ne - ns).num_nanoseconds().unwrap() as f64 * 0.5) as i64;
        // Equality in nanoseconds within rounding slop of a few ns.
        assert!(
            (before_anchor_ns - after_anchor_ns).abs() <= 2,
            "anchor drifted: before={before_anchor_ns}, after={after_anchor_ns}",
        );
    }

    #[test]
    fn zoom_out_widens_span() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = zoom_time_window_at(
            (start, end),
            500.0,
            1000.0,
            2.0,
            Duration::minutes(1).num_nanoseconds().unwrap(),
        );
        let before = (end - start).num_seconds();
        let after = (ne - ns).num_seconds();
        assert!(after > before, "expected wider span after zoom-out");
    }

    #[test]
    fn zoom_in_narrows_span() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = zoom_time_window_at(
            (start, end),
            500.0,
            1000.0,
            0.5,
            Duration::minutes(1).num_nanoseconds().unwrap(),
        );
        let before = (end - start).num_seconds();
        let after = (ne - ns).num_seconds();
        assert!(after < before, "expected narrower span after zoom-in");
    }

    #[test]
    fn zoom_clamps_to_min_10_candles() {
        // Huge zoom-in (factor 0.0001) must bottom out at 10 × candle
        // width.
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let candle_ns = Duration::minutes(1).num_nanoseconds().unwrap();
        let (ns, ne) = zoom_time_window_at((start, end), 500.0, 1000.0, 0.0001, candle_ns);
        let span = (ne - ns).num_nanoseconds().unwrap();
        let min_span = (MIN_VISIBLE_CANDLES as i64) * candle_ns;
        assert!(
            span >= min_span,
            "span {} below 10-candle floor {}",
            span,
            min_span
        );
    }

    #[test]
    fn zoom_clamps_to_max_10_years() {
        // Huge zoom-out factor gets clamped to 10-year maximum.
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let (ns, ne) = zoom_time_window_at(
            (start, end),
            500.0,
            1000.0,
            1_000_000.0,
            Duration::minutes(1).num_nanoseconds().unwrap(),
        );
        let span = ne - ns;
        let max = Duration::days(365 * MAX_ZOOM_OUT_YEARS);
        assert!(span <= max);
    }

    #[test]
    fn zoom_bad_factor_is_noop() {
        let start = utc(2024, 3, 1, 0, 0);
        let end = utc(2024, 3, 2, 0, 0);
        let candle_ns = Duration::minutes(1).num_nanoseconds().unwrap();
        // factor = 0 → noop.
        let (ns, ne) = zoom_time_window_at((start, end), 500.0, 1000.0, 0.0, candle_ns);
        assert_eq!(ns, start);
        assert_eq!(ne, end);
        // factor = NaN → noop.
        let (ns, ne) = zoom_time_window_at((start, end), 500.0, 1000.0, f32::NAN, candle_ns);
        assert_eq!(ns, start);
        assert_eq!(ne, end);
    }

    #[test]
    fn zoom_price_preserves_anchor() {
        let r = PriceRange::new(90.0, 110.0).unwrap();
        let anchor_y = 200.0; // mid-pixel at height 400.
        let height = 400.0;
        let anchor_frac = anchor_y as f64 / height as f64;
        let before = r.high() - anchor_frac * r.span();
        let new = zoom_price_range_at(r, anchor_y, height, 0.5).unwrap();
        let after = new.high() - anchor_frac * new.span();
        assert!(
            (before - after).abs() < 1e-6,
            "anchor price drifted: before={before}, after={after}",
        );
    }

    #[test]
    fn zoom_price_factor_zero_returns_none() {
        let r = PriceRange::new(90.0, 110.0).unwrap();
        assert!(zoom_price_range_at(r, 200.0, 400.0, 0.0).is_none());
    }

    #[test]
    fn zoom_price_nan_anchor_returns_none() {
        let r = PriceRange::new(90.0, 110.0).unwrap();
        assert!(zoom_price_range_at(r, f32::NAN, 400.0, 0.5).is_none());
    }

    // ── auto_scale_price ────────────────────────────────────────────

    fn mk_candle(ts: Timestamp, o: f64, h: f64, l: f64, c: f64) -> Candle {
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

    #[test]
    fn auto_scale_price_fits_visible_high_low_with_5pct_pad() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(start, 100.0, 105.0, 95.0, 100.0));
        s.push(mk_candle(
            start + Duration::minutes(1),
            100.0,
            110.0,
            90.0,
            100.0,
        ));
        let r = auto_scale_price(&s, 0..2).unwrap();
        let span = 110.0 - 90.0;
        let pad = span * 0.05;
        assert!((r.low() - (90.0 - pad)).abs() < 1e-6);
        assert!((r.high() - (110.0 + pad)).abs() < 1e-6);
    }

    #[test]
    fn auto_scale_price_on_empty_range_returns_none() {
        let cal = xnys();
        let s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        assert!(auto_scale_price(&s, 0..0).is_none());
    }

    #[test]
    fn auto_scale_price_degenerate_high_eq_low_pads() {
        // When every candle has h == l, pad by max(0.01, 1% of price).
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(start, 100.0, 100.0, 100.0, 100.0));
        let r = auto_scale_price(&s, 0..1).unwrap();
        // high == low == 100, pad = max(0.01, 0.01*100) = 1.0.
        assert!((r.low() - 99.0).abs() < 1e-6);
        assert!((r.high() - 101.0).abs() < 1e-6);
        assert!(r.span() > 0.0);
    }

    #[test]
    fn auto_scale_price_clamps_out_of_bounds_range() {
        // Visible range wider than series — clamp to series.len().
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(start, 100.0, 105.0, 95.0, 100.0));
        let r = auto_scale_price(&s, 0..100).unwrap();
        // Only candle 0 counted.
        let span = 105.0 - 95.0;
        let pad = span * 0.05;
        assert!((r.low() - (95.0 - pad)).abs() < 1e-6);
        assert!((r.high() - (105.0 + pad)).abs() < 1e-6);
    }
}
