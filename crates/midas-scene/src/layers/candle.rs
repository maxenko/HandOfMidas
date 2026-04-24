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
    /// Alpha multiplier applied to candles whose index is present in
    /// the optional `bright_indices` set (see
    /// [`CandleLayer::with_bright_indices`]). Values above `1.0` clamp
    /// to the u8 ceiling at emit time. Default `1.0` — i.e.
    /// unchanged — so a layer that doesn't opt into the bright
    /// channel behaves identically to one that does but whose set is
    /// empty. Slice 6 of the chart-transition plan: G.ATR selects
    /// non-paranormal bars; those bars render at full alpha, the rest
    /// (dimmed via `pre_market_tint` / `post_market_tint`) fade back.
    pub bright_multiplier: f32,
}

impl Default for CandleStyle {
    fn default() -> Self {
        Self {
            body_width_px: 8.0,
            pre_market_tint: 0.6,
            post_market_tint: 0.8,
            bright_multiplier: 1.0,
        }
    }
}

/// Shared index set of "bright" candles. Populated by the G.ATR
/// indicator layer to highlight the non-paranormal days it averaged;
/// read by [`CandleLayer`] at paint time. `Arc<RwLock<...>>` so the
/// producer and consumer share the same allocation without cloning
/// per frame.
pub type BrightIndices = std::sync::Arc<parking_lot::RwLock<Vec<usize>>>;

/// Candle body + wick layer. Reads an `Arc<RwLock<CandleSeries>>`
/// shared with the driver (single writer, many readers). `paint` takes
/// a read-guard for the scope of the primitive emission and drops it
/// before returning — the guard never escapes `paint` and is never held
/// across an `.await`.
pub struct CandleLayer {
    candles: SharedCandleSeries,
    style: CandleStyle,
    /// Optional bright-index channel. When `Some(idx_set)`, candles
    /// whose zero-based index appears in the set render at
    /// `CandleStyle.bright_multiplier` alpha. Typically populated by
    /// [`super::super::layers::indicator::GerchikAtrLayer`].
    bright_indices: Option<BrightIndices>,
}

impl CandleLayer {
    /// Build a layer over `candles` with `style`. No bright-index
    /// channel — backward-compatible with every pre-slice-6 caller.
    pub fn new(candles: SharedCandleSeries, style: CandleStyle) -> Self {
        Self {
            candles,
            style,
            bright_indices: None,
        }
    }

    /// Build a layer that consults `bright_indices` at paint time.
    ///
    /// The Arc is shared with the producer (G.ATR indicator layer).
    /// Writes on the producer side land on the next paint — no
    /// synchronisation dance; `parking_lot::RwLock` is the cheap
    /// read path.
    ///
    /// Slice 6 addition. Existing `new` / `with_defaults` stay
    /// untouched to preserve backward compatibility — downstream
    /// callers that don't care about the bright channel don't pay
    /// for it.
    pub fn with_bright_indices(
        candles: SharedCandleSeries,
        style: CandleStyle,
        bright_indices: BrightIndices,
    ) -> Self {
        Self {
            candles,
            style,
            bright_indices: Some(bright_indices),
        }
    }

    /// Convenience: build with [`CandleStyle::default`].
    pub fn with_defaults(candles: SharedCandleSeries) -> Self {
        Self::new(candles, CandleStyle::default())
    }

    #[inline]
    pub fn style(&self) -> CandleStyle {
        self.style
    }

    /// Borrow the bright-index channel, if one was attached via
    /// [`Self::with_bright_indices`]. Primarily for tests and
    /// diagnostics.
    #[inline]
    pub fn bright_indices(&self) -> Option<&BrightIndices> {
        self.bright_indices.as_ref()
    }
}

/// Apply a session-based alpha tint plus an optional "bright"
/// boost when the candle's index appears in the shared set.
fn tint(color: [u8; 4], kind: SessionKind, style: &CandleStyle, is_bright: bool) -> [u8; 4] {
    let session_factor = match kind {
        SessionKind::PreMarket => style.pre_market_tint,
        SessionKind::PostMarket => style.post_market_tint,
        _ => 1.0,
    };
    let bright_factor = if is_bright {
        style.bright_multiplier
    } else {
        1.0
    };
    let a = (color[3] as f32 * session_factor * bright_factor).clamp(0.0, 255.0);
    let a = a.round() as u8;
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

        // Snapshot the bright-index set once — we iterate candles with
        // `enumerate` and membership-test each idx. The snapshot (a
        // read-guard held inside the `Option`) is dropped at end-of-
        // scope with `paint`. Cheap even for long series: linear scan
        // against a `Vec<usize>`, typically of size `GATR_LOOKBACK = 7`.
        let bright_guard = self.bright_indices.as_ref().map(|arc| arc.read());

        for (idx, row) in guard.iter().enumerate() {
            let is_bright = bright_guard.as_deref().is_some_and(|bs| bs.contains(&idx));

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
            let body_color = tint(base, row.session_kind(), &self.style, is_bright);
            let wick_color = tint(
                palette.candle_wick,
                row.session_kind(),
                &self.style,
                is_bright,
            );

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
    use midas_axis::{
        ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, TimeAxis, Viewport,
    };
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
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
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
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
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
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
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
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
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
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
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
        let paxis = LinearPriceAxis::new(pr, vp.height_px);
        let fmt = DefaultFormatter::new();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = CandleLayer::with_defaults(s);
        layer.paint(&mut ctx);
        assert_eq!(out.candles.len(), 10);
    }
}
