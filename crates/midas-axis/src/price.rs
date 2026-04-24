//! [`PriceAxis`] — pluggable price-axis projection.
//!
//! Slice 2a of the chart-transition plan. Mirrors the x-axis
//! [`crate::TimeAxis`] pattern so every chart consumer routes y-axis
//! arithmetic through a single trait; the concrete axis
//! (linear / log — log is Phase F) is chosen at chart construction and
//! may be swapped without touching candle data.
//!
//! The trait is SANS-IO: no GPU types, no iced / wgpu coupling, no
//! framework dependencies. It only knows pixels and prices.
//!
//! ## Design decision (plan D5)
//!
//! A trait today with a single [`LinearPriceAxis`] impl is deliberate —
//! log-scale is a deferred non-goal. If log-scale never lands, the
//! trait collapses to a struct; until then the trait preserves the
//! insertion seam.

use crate::{PriceRange, SnapDirection};

/// Pluggable price-axis projection. Every chart consumer routes y-axis
/// arithmetic through this trait.
///
/// Convention follows graphics Y-axis semantics: `y = 0` sits at the
/// TOP of the viewport and maps to `range.high()`; `y = height_px`
/// sits at the bottom and maps to `range.low()`.
///
/// All four projection methods ([`to_y`], [`from_y`],
/// [`from_y_snapped`], [`height_px`]) are `O(1)` and never allocate.
///
/// [`to_y`]: PriceAxis::to_y
/// [`from_y`]: PriceAxis::from_y
/// [`from_y_snapped`]: PriceAxis::from_y_snapped
/// [`height_px`]: PriceAxis::height_px
#[allow(clippy::wrong_self_convention)] // `from_y`/`from_y_snapped` are
                                        // axis-direction names, not
                                        // type-constructor conversions.
                                        // Matches the convention on
                                        // [`crate::TimeAxis`].
pub trait PriceAxis: Send + Sync {
    /// Price to pixel. Does NOT clamp — callers that need clamping do
    /// so on their side so layers that render partial off-screen
    /// elements (price lines above/below the viewport) retain the
    /// exact y coordinate.
    fn to_y(&self, price: f64) -> f32;

    /// Pixel to price. Returns `None` iff `y` is not finite or the
    /// axis has a non-positive height (degenerate viewport mid-
    /// animation). In-range and out-of-range finite y values both
    /// return `Some` — extrapolation is the caller's concern.
    fn from_y(&self, y: f32) -> Option<f64>;

    /// Pixel to price with snap. Always returns a price. The `bool`
    /// flag is `true` iff a snap was performed (i.e. `from_y` would
    /// have returned `None` for a non-finite `y`, or the snap moved
    /// the raw value onto a range edge per `dir`). For a
    /// [`LinearPriceAxis`] snap is a no-op on finite inputs and the
    /// flag is `false`; kept here for parity with
    /// [`crate::TimeAxis::from_x_snapped`] and because log-axis impls
    /// will exercise it.
    fn from_y_snapped(&self, y: f32, dir: SnapDirection) -> (f64, bool);

    /// Viewport height in pixels.
    fn height_px(&self) -> f32;

    /// The price range this axis currently covers.
    fn range(&self) -> PriceRange;
}

/// Linear (affine) price axis. The most common axis shape —
/// equities + futures + crypto all ship on this by default.
///
/// Maps `range.high()` → `y = 0` and `range.low()` → `y = height_px`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LinearPriceAxis {
    range: PriceRange,
    height_px: f32,
}

impl LinearPriceAxis {
    /// Build a new linear price axis.
    ///
    /// `height_px` should be positive; a zero/negative/non-finite
    /// value is tolerated (the axis still constructs) but `to_y` /
    /// `from_y` will degenerate to returning the range edge / `None`
    /// respectively. This mirrors [`crate::ContinuousAxis`]'s defensive
    /// posture during window-resize animations.
    pub fn new(range: PriceRange, height_px: f32) -> Self {
        tracing::debug!(
            target: "midas_axis::price::new",
            low = range.low(),
            high = range.high(),
            height_px,
            "construct LinearPriceAxis",
        );
        Self { range, height_px }
    }

    /// Replace the price range (auto-scale / user zoom).
    pub fn set_range(&mut self, range: PriceRange) {
        tracing::debug!(
            target: "midas_axis::price::set_range",
            old_low = self.range.low(),
            old_high = self.range.high(),
            new_low = range.low(),
            new_high = range.high(),
            "replace range",
        );
        self.range = range;
    }

    /// Replace the viewport height (window resize).
    pub fn set_height_px(&mut self, height_px: f32) {
        self.height_px = height_px;
    }
}

impl PriceAxis for LinearPriceAxis {
    #[inline]
    fn to_y(&self, price: f64) -> f32 {
        let span = self.range.span();
        if span <= 0.0 || !self.height_px.is_finite() || self.height_px <= 0.0 {
            // Degenerate axis — caller's slot up top.
            return 0.0;
        }
        let frac = (self.range.high() - price) / span;
        (frac * self.height_px as f64) as f32
    }

    #[inline]
    fn from_y(&self, y: f32) -> Option<f64> {
        if !y.is_finite() {
            return None;
        }
        if !self.height_px.is_finite() || self.height_px <= 0.0 {
            return None;
        }
        let span = self.range.span();
        let frac = (y as f64) / (self.height_px as f64);
        Some(self.range.high() - frac * span)
    }

    #[inline]
    fn from_y_snapped(&self, y: f32, dir: SnapDirection) -> (f64, bool) {
        // Linear axis: non-finite inputs snap to a range edge chosen
        // by `dir`. Finite inputs pass straight through `from_y` with
        // no snap.
        if let Some(p) = self.from_y(y) {
            return (p, false);
        }
        let snapped = match dir {
            SnapDirection::Forward => self.range.high(),
            SnapDirection::Backward => self.range.low(),
            // Nearest: no meaningful nearer-edge signal on a NaN y —
            // pick `high` as a stable default matching Forward.
            SnapDirection::Nearest => self.range.high(),
        };
        (snapped, true)
    }

    #[inline]
    fn height_px(&self) -> f32 {
        self.height_px
    }

    #[inline]
    fn range(&self) -> PriceRange {
        self.range
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> PriceRange {
        PriceRange::new(90.0, 110.0).unwrap()
    }

    #[test]
    fn to_y_high_maps_to_zero() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        assert!((ax.to_y(110.0) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn to_y_low_maps_to_height() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        assert!((ax.to_y(90.0) - 400.0).abs() < 1e-3);
    }

    #[test]
    fn to_y_midpoint() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        assert!((ax.to_y(100.0) - 200.0).abs() < 1e-3);
    }

    #[test]
    fn from_y_round_trips_for_interior_prices() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        for &p in &[91.0, 95.5, 100.0, 103.25, 109.75] {
            let y = ax.to_y(p);
            let back = ax.from_y(y).unwrap();
            assert!(
                (p - back).abs() < 1e-3,
                "round-trip failed for {p}: got {back}",
            );
        }
    }

    #[test]
    fn from_y_extrapolates_outside_viewport() {
        // y = -100 is above the viewport; result should extrapolate
        // past `high`.
        let ax = LinearPriceAxis::new(pr(), 400.0);
        let p = ax.from_y(-100.0).unwrap();
        assert!(p > ax.range().high(), "expected p > 110 got {p}");

        let p2 = ax.from_y(500.0).unwrap();
        assert!(p2 < ax.range().low(), "expected p < 90 got {p2}");
    }

    #[test]
    fn from_y_nan_returns_none() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        assert!(ax.from_y(f32::NAN).is_none());
        assert!(ax.from_y(f32::INFINITY).is_none());
        assert!(ax.from_y(f32::NEG_INFINITY).is_none());
    }

    #[test]
    fn from_y_zero_height_returns_none() {
        let ax = LinearPriceAxis::new(pr(), 0.0);
        assert!(ax.from_y(100.0).is_none());
    }

    #[test]
    fn to_y_zero_height_returns_zero() {
        let ax = LinearPriceAxis::new(pr(), 0.0);
        assert_eq!(ax.to_y(100.0), 0.0);
    }

    #[test]
    fn from_y_snapped_forward_returns_high_for_nan() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        let (p, snapped) = ax.from_y_snapped(f32::NAN, SnapDirection::Forward);
        assert_eq!(p, 110.0);
        assert!(snapped);
    }

    #[test]
    fn from_y_snapped_backward_returns_low_for_nan() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        let (p, snapped) = ax.from_y_snapped(f32::NAN, SnapDirection::Backward);
        assert_eq!(p, 90.0);
        assert!(snapped);
    }

    #[test]
    fn from_y_snapped_nearest_defaults_to_high() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        let (p, snapped) = ax.from_y_snapped(f32::NAN, SnapDirection::Nearest);
        assert_eq!(p, 110.0);
        assert!(snapped);
    }

    #[test]
    fn from_y_snapped_finite_no_snap() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        let (p, snapped) = ax.from_y_snapped(200.0, SnapDirection::Forward);
        assert!(!snapped, "finite input must not report a snap");
        assert!((p - 100.0).abs() < 1e-3);
    }

    #[test]
    fn accessors_expose_range_and_height() {
        let ax = LinearPriceAxis::new(pr(), 400.0);
        assert_eq!(ax.range(), pr());
        assert_eq!(ax.height_px(), 400.0);
    }

    #[test]
    fn set_range_and_height_mutate_in_place() {
        let mut ax = LinearPriceAxis::new(pr(), 400.0);
        let new_range = PriceRange::new(50.0, 60.0).unwrap();
        ax.set_range(new_range);
        ax.set_height_px(800.0);
        assert_eq!(ax.range(), new_range);
        assert_eq!(ax.height_px(), 800.0);
        // Midpoint 55 → y = 400.0 at height = 800.
        assert!((ax.to_y(55.0) - 400.0).abs() < 1e-3);
    }

    #[test]
    fn trait_object_send_sync() {
        fn takes_send_sync<T: Send + Sync + ?Sized>(_: &T) {}
        let ax = LinearPriceAxis::new(pr(), 400.0);
        let dynref: &dyn PriceAxis = &ax;
        takes_send_sync(dynref);
    }
}
