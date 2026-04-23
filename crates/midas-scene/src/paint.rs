//! [`PaintContext`] — the sans-IO emitter scope handed to every layer.
//!
//! Per R2-NB-3 the context bundles the three projection pieces (axis,
//! viewport, price range), the palette the layer may sample from, and
//! a mutable reference to the [`ScenePrimitives`] buffer the layer
//! appends into. Layers never hold these themselves — they receive them
//! for the duration of one `paint` call.

use midas_axis::{PriceRange, TimeAxis, Viewport};

use crate::primitives::ScenePrimitives;
use crate::ThemePalette;

/// Scope passed to [`SceneLayer::paint`](crate::SceneLayer::paint).
///
/// Layer paint bodies are pure functions of `(axis, viewport,
/// price_range, palette, own-state) → write primitives to `out``. No
/// GPU types. No iced types. Mockable by hand for tests.
pub struct PaintContext<'a> {
    pub axis: &'a dyn TimeAxis,
    pub viewport: Viewport,
    pub price_range: PriceRange,
    pub palette: &'a ThemePalette,
    pub out: &'a mut ScenePrimitives,
}

impl PaintContext<'_> {
    /// Map a price to a y-pixel. `y = 0` sits at the top of the
    /// viewport (conventional graphics coordinate) and corresponds to
    /// `price_range.high`; `y = viewport.height_px` sits at the bottom
    /// and corresponds to `price_range.low`.
    ///
    /// Does NOT clamp — callers that need clamping do so on their side,
    /// so layers that render partial off-screen elements (e.g. price
    /// lines above the viewport) retain the exact y coordinate.
    #[inline]
    pub fn price_to_y(&self, price: f64) -> f32 {
        let span = self.price_range.span();
        if span <= 0.0 {
            return 0.0;
        }
        let frac = (self.price_range.high() - price) / span;
        (frac * self.viewport.height_px as f64) as f32
    }

    /// Inverse of [`price_to_y`](Self::price_to_y). Routes a mouse y
    /// coordinate back into price space.
    #[inline]
    pub fn y_to_price(&self, y: f32) -> f64 {
        let span = self.price_range.span();
        if self.viewport.height_px <= 0.0 {
            return self.price_range.high();
        }
        let frac = (y as f64) / (self.viewport.height_px as f64);
        self.price_range.high() - frac * span
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn ctx_harness() -> (ContinuousAxis, PriceRange, Viewport, ThemePalette) {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        (axis, pr, vp, ThemePalette::dark_default())
    }

    #[test]
    fn price_to_y_high_maps_to_zero() {
        let (axis, pr, vp, pal) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        assert!((ctx.price_to_y(110.0) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn price_to_y_low_maps_to_height() {
        let (axis, pr, vp, pal) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        assert!((ctx.price_to_y(90.0) - 400.0).abs() < 1e-3);
    }

    #[test]
    fn price_to_y_midpoint() {
        let (axis, pr, vp, pal) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        assert!((ctx.price_to_y(100.0) - 200.0).abs() < 1e-3);
    }

    #[test]
    fn y_to_price_round_trips() {
        let (axis, pr, vp, pal) = ctx_harness();
        let mut out = ScenePrimitives::default();
        let ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        for &p in &[91.0, 95.5, 100.0, 103.25, 109.75] {
            let y = ctx.price_to_y(p);
            let back = ctx.y_to_price(y);
            assert!((p - back).abs() < 1e-3, "p={p} back={back}");
        }
    }
}
