//! [`VolumeLayer`] — bottom-pane volume bars inside the main chart.

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::layers::candle::SharedCandleSeries;
use crate::paint::PaintContext;
use crate::primitives::QuadInstance;

/// Visual knobs for [`VolumeLayer`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct VolumeStyle {
    /// Fraction of the viewport height consumed by the volume strip at
    /// the bottom. `0.20` = 20% of the viewport. Per R2-G-1 the MVP
    /// renders volume in-pane; multi-pane split is deferred.
    pub bottom_fraction: f32,
    /// Width of each volume bar in pixels.
    pub bar_width_px: f32,
}

impl Default for VolumeStyle {
    fn default() -> Self {
        Self {
            bottom_fraction: 0.20,
            bar_width_px: 8.0,
        }
    }
}

/// Bottom-strip volume bars. Each bar's height scales linearly with
/// `volume / max_volume` over the series; colour matches the candle
/// direction (up-bar → `palette.candle_up`; down-bar →
/// `palette.candle_down`). `paint` takes a read-guard on the shared
/// series and drops it before returning (the guard never escapes
/// `paint` and is never held across `.await`).
pub struct VolumeLayer {
    candles: SharedCandleSeries,
    style: VolumeStyle,
}

impl VolumeLayer {
    pub fn new(candles: SharedCandleSeries, style: VolumeStyle) -> Self {
        Self { candles, style }
    }

    pub fn with_defaults(candles: SharedCandleSeries) -> Self {
        Self::new(candles, VolumeStyle::default())
    }

    #[inline]
    pub fn style(&self) -> VolumeStyle {
        self.style
    }
}

impl SceneLayer for VolumeLayer {
    fn id(&self) -> LayerId {
        LayerId("volume")
    }

    fn z(&self) -> LayerZ {
        LayerZ::VOLUME
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        // Short-lived read-guard. Released at end-of-scope; `paint` is
        // synchronous so there's no `.await` boundary to worry about.
        let guard = self.candles.read();
        if guard.is_empty() {
            return;
        }
        let max_volume = guard.iter().map(|r| r.volume()).max().unwrap_or(0).max(1);
        let strip_h = ctx.viewport.height_px * self.style.bottom_fraction;
        let strip_top = ctx.viewport.height_px - strip_h;

        for row in guard.iter() {
            let x_center = ctx.axis.to_x(row.ts_open());
            let frac = (row.volume() as f64 / max_volume as f64) as f32;
            let h = (frac * strip_h).max(0.0);
            let y = strip_top + (strip_h - h);
            let color = if row.close() >= row.open() {
                ctx.palette.candle_up
            } else {
                ctx.palette.candle_down
            };
            ctx.out.quads.push(QuadInstance {
                x: x_center - self.style.bar_width_px * 0.5,
                y,
                w: self.style.bar_width_px,
                h,
                color,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_bars::{Candle, CandleSeries, Completeness, Ohlcv, Symbol};
    use midas_calendar::{xnys, BarPeriod, Timestamp};
    use parking_lot::RwLock;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    fn mk(ts: Timestamp, price: f64, vol: u64) -> Candle {
        let cal = xnys();
        let sym = Symbol::new("SPY", cal.id());
        let session = cal.classify(ts);
        let window = cal.bar_window(ts, BarPeriod::m1()).unwrap();
        let ohlcv =
            Ohlcv::new(price, price + 0.1, price - 0.1, price + 0.05, vol, 1, None).unwrap();
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
    fn emits_n_quads_for_n_bars() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30);
        for i in 0..7 {
            s.push(mk(
                start + chrono::Duration::minutes(i),
                100.0,
                100 + i as u64 * 10,
            ));
        }
        let s: SharedCandleSeries = Arc::new(RwLock::new(s));
        let axis = ContinuousAxis::new(start, start + chrono::Duration::hours(1), 1000.0).unwrap();
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
        VolumeLayer::with_defaults(s).paint(&mut ctx);
        assert_eq!(out.quads.len(), 7);
    }

    #[test]
    fn bar_height_scales_with_volume_ratio() {
        let cal = xnys();
        let mut s = CandleSeries::new(cal.id(), BarPeriod::m1(), Symbol::new("SPY", cal.id()));
        let start = utc(2024, 1, 17, 14, 30);
        s.push(mk(start, 100.0, 100)); // min
        s.push(mk(start + chrono::Duration::minutes(1), 100.0, 1_000)); // max
        let s: SharedCandleSeries = Arc::new(RwLock::new(s));
        let axis = ContinuousAxis::new(start, start + chrono::Duration::hours(1), 1000.0).unwrap();
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
        VolumeLayer::with_defaults(s).paint(&mut ctx);
        assert!(out.quads[1].h > out.quads[0].h);
        // Strip height is 20% of 400 = 80 px; max-volume bar fills it.
        assert!((out.quads[1].h - 80.0).abs() < 1e-3);
    }

    #[test]
    fn empty_series_emits_nothing() {
        let cal = xnys();
        let s: SharedCandleSeries = Arc::new(RwLock::new(CandleSeries::new(
            cal.id(),
            BarPeriod::m1(),
            Symbol::new("SPY", cal.id()),
        )));
        let axis = ContinuousAxis::new(utc(2024, 1, 17, 14, 30), utc(2024, 1, 17, 15, 30), 1000.0)
            .unwrap();
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
        VolumeLayer::with_defaults(s).paint(&mut ctx);
        assert_eq!(out.quads.len(), 0);
    }
}
