//! [`CrosshairLayer`] — mouse-following horizontal + vertical guides.

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::LineInstance;

/// Two thin lines tracking the mouse, emitted only when `position`
/// is `Some`. The input layer (S9) updates `position` on pointer-move;
/// the chart drops `position = None` on pointer-exit.
#[derive(Copy, Clone, Debug, Default)]
pub struct CrosshairLayer {
    pub position: Option<(f32, f32)>,
    pub line_width_px: f32,
}

impl CrosshairLayer {
    pub fn new() -> Self {
        Self {
            position: None,
            line_width_px: 1.0,
        }
    }

    pub fn with_position(position: (f32, f32)) -> Self {
        Self {
            position: Some(position),
            line_width_px: 1.0,
        }
    }
}

impl SceneLayer for CrosshairLayer {
    fn id(&self) -> LayerId {
        LayerId("crosshair")
    }

    fn z(&self) -> LayerZ {
        LayerZ::CROSSHAIR
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let Some((x, y)) = self.position else {
            return;
        };
        let color = ctx.palette.crosshair;
        let w = self.line_width_px.max(0.5);
        // Horizontal arm.
        ctx.out.lines.push(LineInstance {
            x0: 0.0,
            y0: y,
            x1: ctx.viewport.width_px,
            y1: y,
            width_px: w,
            color,
        });
        // Vertical arm.
        ctx.out.lines.push(LineInstance {
            x0: x,
            y0: 0.0,
            x1: x,
            y1: ctx.viewport.height_px,
            width_px: w,
            color,
        });
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn harness() -> (ContinuousAxis, PriceRange, Viewport, ThemePalette) {
        let axis =
            ContinuousAxis::new(ts(2024, 1, 1, 0, 0, 0), ts(2024, 1, 2, 0, 0, 0), 1000.0).unwrap();
        let pr = PriceRange::new(90.0, 110.0).unwrap();
        let vp = Viewport::new(1000.0, 400.0);
        (axis, pr, vp, ThemePalette::dark_default())
    }

    #[test]
    fn none_position_emits_zero_lines() {
        let (axis, pr, vp, pal) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        CrosshairLayer::new().paint(&mut ctx);
        assert_eq!(out.lines.len(), 0);
    }

    #[test]
    fn some_position_emits_two_lines() {
        let (axis, pr, vp, pal) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        CrosshairLayer::with_position((500.0, 200.0)).paint(&mut ctx);
        assert_eq!(out.lines.len(), 2);
        // Horizontal arm: y0 == y1.
        assert!((out.lines[0].y0 - out.lines[0].y1).abs() < 1e-3);
        // Vertical arm: x0 == x1.
        assert!((out.lines[1].x0 - out.lines[1].x1).abs() < 1e-3);
    }

    #[test]
    fn arms_span_full_viewport() {
        let (axis, pr, vp, pal) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            out: &mut out,
        };
        CrosshairLayer::with_position((300.0, 100.0)).paint(&mut ctx);
        // Horizontal: x0==0, x1==width.
        assert_eq!(out.lines[0].x0, 0.0);
        assert!((out.lines[0].x1 - 1000.0).abs() < 1e-3);
        // Vertical: y0==0, y1==height.
        assert_eq!(out.lines[1].y0, 0.0);
        assert!((out.lines[1].y1 - 400.0).abs() < 1e-3);
    }
}
