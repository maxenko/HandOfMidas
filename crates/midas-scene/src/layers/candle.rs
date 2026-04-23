//! [`CandleLayer`] — OHLC candles tinted by session.

use std::sync::Arc;

use midas_bars::{CandleSeries, SessionKind};
use parking_lot::RwLock;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::CandleInstance;

/// Shared candle-series handle type. The layer holds a read-only
/// handle; the driver writes under the same lock. Typedef alias so
/// callers building layers from a driver-side series don't have to
/// repeat the full path at every call site.
pub type SharedCandleSeries = Arc<RwLock<CandleSeries>>;

/// Visual knobs for [`CandleLayer`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CandleStyle {
    /// Body width in pixels. 8 px matches the existing `midas-app`
    /// default at 1x DPI.
    pub body_width_px: f32,
    /// Multiplier applied to `color` alpha for `PreMarket` candles.
    /// 1.0 = full colour; 0.5 = half-alpha.
    pub pre_market_tint: f32,
    /// Multiplier applied to `color` alpha for `PostMarket` candles.
    pub post_market_tint: f32,
}

impl Default for CandleStyle {
    fn default() -> Self {
        Self {
            body_width_px: 8.0,
            pre_market_tint: 0.6,
            post_market_tint: 0.8,
        }
    }
}

/// Candle body + wick layer. Reads an `Arc<RwLock<CandleSeries>>`
/// shared with the driver (single writer, many readers). `paint` takes
/// a read-guard for the scope of the primitive emission and drops it
/// before returning — the guard never escapes `paint` and is never held
/// across an `.await`.
pub struct CandleLayer {
    candles: SharedCandleSeries,
    style: CandleStyle,
}

impl CandleLayer {
    /// Build a layer over `candles` with `style`.
    pub fn new(candles: SharedCandleSeries, style: CandleStyle) -> Self {
        Self { candles, style }
    }

    /// Convenience: build with [`CandleStyle::default`].
    pub fn with_defaults(candles: SharedCandleSeries) -> Self {
        Self::new(candles, CandleStyle::default())
    }

    #[inline]
    pub fn style(&self) -> CandleStyle {
        self.style
    }
}

/// Apply a session-based alpha tint.
fn tint(color: [u8; 4], kind: SessionKind, style: &CandleStyle) -> [u8; 4] {
    let factor = match kind {
        SessionKind::PreMarket => style.pre_market_tint,
        SessionKind::PostMarket => style.post_market_tint,
        _ => 1.0,
    };
    let a = (color[3] as f32 * factor).round().clamp(0.0, 255.0) as u8;
    [color[0], color[1], color[2], a]
}

impl SceneLayer for CandleLayer {
    fn id(&self) -> LayerId {
        LayerId("candles")
    }

    fn z(&self) -> LayerZ {
        LayerZ::CANDLE
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        // Take a short-lived read-guard. `paint` is a synchronous
        // function — no `.await` touches `guard`, so the
        // `await_holding_lock` concern doesn't apply. The guard is
        // dropped at end-of-scope when `paint` returns.
        let guard = self.candles.read();
        if guard.is_empty() {
            return;
        }
        let axis = ctx.axis;
        let palette = ctx.palette;
        let width = self.style.body_width_px;

        for row in guard.iter() {
            let x_center = axis.to_x(row.ts_open());
            let high_px = ctx.price_to_y(row.high());
            let low_px = ctx.price_to_y(row.low());
            let open_px = ctx.price_to_y(row.open());
            let close_px = ctx.price_to_y(row.close());

            let is_up = row.close() >= row.open();
            let base = if is_up {
                palette.candle_up
            } else {
                palette.candle_down
            };
            let body_color = tint(base, row.session_kind(), &self.style);
            let wick_color = tint(palette.candle_wick, row.session_kind(), &self.style);

            ctx.out.candles.push(CandleInstance {
                x_center,
                width_px: width,
                open_px,
                high_px,
                low_px,
                close_px,
                color: body_color,
                wick_color,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, TimeAxis, Viewport};
    use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{xnys, BarPeriod, Timestamp};

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

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

    fn harness(len: usize) -> (SharedCandleSeries, ContinuousAxis, PriceRange, Viewport) {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30); // 09:30 ET → Regular
        for i in 0..len {
            let ts = start + chrono::Duration::minutes(i as i64);
            let price = 100.0 + (i as f64) * 0.1;
            s.push(mk_candle(ts, price, price + 0.2, price - 0.2, price + 0.1));
        }
        let axis = ContinuousAxis::new(start, start + chrono::Duration::hours(1), 1000.0).unwrap();
        let pr = PriceRange::new(95.0, 120.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        (Arc::new(RwLock::new(s)), axis, pr, vp)
    }

    #[test]
    fn empty_series_emits_zero_candles() {
        let cal = xnys();
        let s: SharedCandleSeries = Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            Symbol::new("SPY", cal.id()),
        )));
        let axis = ContinuousAxis::new(utc(2024, 1, 17, 14, 30), utc(2024, 1, 17, 15, 30), 1000.0)
            .unwrap();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        assert_eq!(out.candles.len(), 0);
    }

    #[test]
    fn one_bar_emits_one_candle_at_axis_x() {
        let (s, axis, pr, vp) = harness(1);
        let expected_x = axis.to_x(s.read().at(0).unwrap().ts_open());
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        assert_eq!(out.candles.len(), 1);
        assert!((out.candles[0].x_center - expected_x).abs() < 1e-3);
    }

    #[test]
    fn up_bars_pick_candle_up_color() {
        // Explicit up-bar: close > open.
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(ts, 100.0, 101.0, 99.5, 100.9));
        let s: SharedCandleSeries = Arc::new(RwLock::new(s));
        let axis = ContinuousAxis::new(ts, ts + chrono::Duration::hours(1), 1000.0).unwrap();
        let pr = PriceRange::new(95.0, 105.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        // For a Regular session the tint is 1.0, so alpha is unchanged.
        assert_eq!(out.candles[0].color[0..3], pal.candle_up[0..3]);
    }

    #[test]
    fn down_bars_pick_candle_down_color() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let ts = utc(2024, 1, 17, 14, 30);
        s.push(mk_candle(ts, 100.5, 100.6, 99.5, 99.9));
        let s: SharedCandleSeries = Arc::new(RwLock::new(s));
        let axis = ContinuousAxis::new(ts, ts + chrono::Duration::hours(1), 1000.0).unwrap();
        let pr = PriceRange::new(95.0, 105.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        assert_eq!(out.candles[0].color[0..3], pal.candle_down[0..3]);
    }

    #[test]
    fn pre_market_dims_alpha() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        // 13:00 UTC = 08:00 ET → PreMarket.
        let ts = utc(2024, 1, 17, 13, 0);
        s.push(mk_candle(ts, 100.0, 100.5, 99.5, 100.3));
        let s: SharedCandleSeries = Arc::new(RwLock::new(s));
        let axis = ContinuousAxis::new(ts, ts + chrono::Duration::hours(1), 1000.0).unwrap();
        let pr = PriceRange::new(95.0, 105.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        assert!(out.candles[0].color[3] < pal.candle_up[3]);
    }

    #[test]
    fn emits_one_candle_per_bar() {
        let (s, axis, pr, vp) = harness(10);
        let pal = ThemePalette::dark_default();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        assert_eq!(out.candles.len(), 10);
    }
}
