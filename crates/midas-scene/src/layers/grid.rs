//! [`GridLayer`] — time + price gridlines.

use midas_axis::TickDensity;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::LineInstance;

/// Visual knobs for [`GridLayer`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GridStyle {
    pub line_width_px: f32,
    pub tick_density: TickDensity,
    /// Approximate pixel spacing between horizontal price gridlines.
    /// The layer picks a "nice" price step so each gridline lands on a
    /// human-friendly value.
    pub price_step_target_px: f32,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self {
            line_width_px: 1.0,
            tick_density: TickDensity::Normal,
            price_step_target_px: 80.0,
        }
    }
}

/// Grid of vertical time-ticks and horizontal price-ticks. Issues no
/// text — axis labels are drawn by a separate (future) axis-label
/// layer; this layer is purely the background grid.
pub struct GridLayer {
    style: GridStyle,
}

impl GridLayer {
    pub fn new(style: GridStyle) -> Self {
        Self { style }
    }

    pub fn with_defaults() -> Self {
        Self::new(GridStyle::default())
    }

    #[inline]
    pub fn style(&self) -> GridStyle {
        self.style
    }
}

impl SceneLayer for GridLayer {
    fn id(&self) -> LayerId {
        LayerId("grid")
    }

    fn z(&self) -> LayerZ {
        LayerZ::GRID
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let color = ctx.palette.grid;
        let w = self.style.line_width_px;

        // Vertical time gridlines.
        for tick in ctx.axis.ticks(self.style.tick_density) {
            ctx.out.lines.push(LineInstance {
                x0: tick.x,
                y0: 0.0,
                x1: tick.x,
                y1: ctx.viewport.height_px,
                width_px: w,
                color,
            });
        }

        // Horizontal price gridlines at "nice" round steps.
        let span = ctx.price_range.span();
        if span > 0.0 && ctx.viewport.height_px > 0.0 {
            let target_steps = (ctx.viewport.height_px / self.style.price_step_target_px).max(1.0);
            let raw_step = span / target_steps as f64;
            let step = nice_step(raw_step);
            let first = (ctx.price_range.low() / step).floor() * step;
            let mut p = first;
            // guard against pathological ranges (step underflows)
            let max_iters = 1024;
            let mut iters = 0;
            while p <= ctx.price_range.high() && iters < max_iters {
                if p >= ctx.price_range.low() {
                    let y = ctx.price_to_y(p);
                    ctx.out.lines.push(LineInstance {
                        x0: 0.0,
                        y0: y,
                        x1: ctx.viewport.width_px,
                        y1: y,
                        width_px: w,
                        color,
                    });
                }
                p += step;
                iters += 1;
            }
        }
    }
}

/// Round `raw` up to a nice 1/2/5 × 10^k step.
fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let mantissa = raw / base;
    let nice_mantissa = if mantissa <= 1.0 {
        1.0
    } else if mantissa <= 2.0 {
        2.0
    } else if mantissa <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice_mantissa * base
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, TimeAxis, Viewport};
    use midas_calendar::Timestamp;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn emits_at_least_one_time_tick_line() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
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
        GridLayer::with_defaults().paint(&mut ctx);
        // Expect a mix of time-ticks (vertical) and price-ticks
        // (horizontal) — separate by orientation.
        let vertical = out
            .lines
            .iter()
            .filter(|l| (l.x0 - l.x1).abs() < 1e-3)
            .count();
        assert!(vertical >= 1, "expected at least one vertical gridline");
    }

    #[test]
    fn emits_horizontal_price_gridlines() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
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
        GridLayer::with_defaults().paint(&mut ctx);
        let horiz = out
            .lines
            .iter()
            .filter(|l| (l.y0 - l.y1).abs() < 1e-3)
            .count();
        assert!(horiz >= 2, "expected several horizontal price gridlines");
    }

    #[test]
    fn tick_count_matches_axis_ticks() {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
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
        let expected_ticks = axis.ticks(TickDensity::Normal).len();
        GridLayer::with_defaults().paint(&mut ctx);
        let vertical = out
            .lines
            .iter()
            .filter(|l| (l.x0 - l.x1).abs() < 1e-3)
            .count();
        assert_eq!(vertical, expected_ticks);
    }

    #[test]
    fn nice_step_picks_1_2_5_scale() {
        assert!((nice_step(0.8) - 1.0).abs() < 1e-9);
        assert!((nice_step(1.7) - 2.0).abs() < 1e-9);
        assert!((nice_step(4.0) - 5.0).abs() < 1e-9);
        assert!((nice_step(6.0) - 10.0).abs() < 1e-9);
        assert!((nice_step(47.0) - 50.0).abs() < 1e-9);
    }
}
