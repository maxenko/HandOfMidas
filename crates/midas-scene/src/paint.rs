//! [`PaintContext`] — the sans-IO emitter scope handed to every layer.
//!
//! Per R2-NB-3 the context bundles the three projection pieces (axis,
//! viewport, price range), the palette the layer may sample from, and
//! a mutable reference to the [`ScenePrimitives`] buffer the layer
//! appends into. Layers never hold these themselves — they receive them
//! for the duration of one `paint` call.

use midas_axis::{LabelFormatter, PriceAxis, PriceRange, TimeAxis, Viewport};

use crate::primitives::ScenePrimitives;
use crate::ThemePalette;

/// Scope passed to [`SceneLayer::paint`](crate::SceneLayer::paint).
///
/// Layer paint bodies are pure functions of `(axis, viewport,
/// price_range, palette, own-state) → write primitives to `out``. No
/// GPU types. No iced types. Mockable by hand for tests.
///
/// Slice 2a widened the context: every layer receives a `&dyn
/// PriceAxis` + `&dyn LabelFormatter` so y-projection and label
/// rendering route through the shared projection surfaces without
/// each layer reimplementing them.
pub struct PaintContext<'a> {
    pub axis: &'a dyn TimeAxis,
    pub viewport: Viewport,
    pub price_range: PriceRange,
    pub palette: &'a ThemePalette,
    /// Pluggable price-axis projection (slice 2a). Delegates
    /// [`PaintContext::price_to_y`] / [`PaintContext::y_to_price`].
    pub price_axis: &'a dyn PriceAxis,
    /// Shared label formatter (slice 2a). Layers route every price +
    /// time label through this so locale / precision policy lands in
    /// one place.
    pub formatter: &'a dyn LabelFormatter,
    pub out: &'a mut ScenePrimitives,
}

impl PaintContext<'_> {
    /// Map a price to a y-pixel. `y = 0` sits at the top of the
    /// viewport (conventional graphics coordinate) and corresponds to
    /// `price_range.high`; `y = viewport.height_px` sits at the bottom
    /// and corresponds to `price_range.low`.
    ///
    /// Delegates to [`PriceAxis::to_y`] on the context's price axis
    /// (slice 2a); keeps the legacy method name for layer code that
    /// predates the axis plumbing.
    ///
    /// Does NOT clamp — callers that need clamping do so on their side,
    /// so layers that render partial off-screen elements (e.g. price
    /// lines above the viewport) retain the exact y coordinate.
    #[inline]
    pub fn price_to_y(&self, price: f64) -> f32 {
        self.price_axis.to_y(price)
    }

    /// Inverse of [`price_to_y`](Self::price_to_y). Routes a mouse y
    /// coordinate back into price space.
    ///
    /// Delegates to [`PriceAxis::from_y`] and falls back to
    /// `price_range.high()` when the axis reports a degenerate
    /// viewport — matches pre-slice-2a behaviour so callers that
    /// previously assumed an infallible result don't regress.
    #[inline]
    pub fn y_to_price(&self, y: f32) -> f64 {
        self.price_axis
            .from_y(y)
            .unwrap_or_else(|| self.price_range.high())
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn ctx_harness() -> (
        ContinuousAxis,
        LinearPriceAxis,
        PriceRange,
        Viewport,
        ThemePalette,
        DefaultFormatter,
    ) {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        (
            axis,
            paxis,
            pr,
            vp,
            ThemePalette::dark_default(),
            DefaultFormatter::new(),
        )
    }

    #[test]
    fn price_to_y_high_maps_to_zero() {
        let (axis, paxis, pr, vp, pal, fmt) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        assert!((ctx.price_to_y(110.0) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn price_to_y_low_maps_to_height() {
        let (axis, paxis, pr, vp, pal, fmt) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        assert!((ctx.price_to_y(90.0) - 400.0).abs() < 1e-3);
    }

    #[test]
    fn price_to_y_midpoint() {
        let (axis, paxis, pr, vp, pal, fmt) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        assert!((ctx.price_to_y(100.0) - 200.0).abs() < 1e-3);
    }

    #[test]
    fn y_to_price_round_trips() {
        let (axis, paxis, pr, vp, pal, fmt) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        for &p in &[91.0, 95.5, 100.0, 103.25, 109.75] {
            let y = ctx.price_to_y(p);
            let back = ctx.y_to_price(y);
            assert!((p - back).abs() < 1e-3, "p={p} back={back}");
        }
    }
}
