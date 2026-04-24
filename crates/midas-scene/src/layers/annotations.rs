//! Annotation layers — per R2-NM-4 each concrete, NOT a generic
//! `AnnotationLayer`. Each visual kind owns its own state machine and
//! its own `LayerZ` slot.
//!
//! Provided here:
//! - [`OrderBracketLayer`] — three-leg brackets (entry + optional TP + SL).
//! - [`PriceLineLayer`] — labelled horizontal lines.
//! - [`LevelLayer`] — named price levels (dashed / alpha-reduced visual
//!   differentiator for MVP).
//! - [`DecoratorLayer`] — stub placeholder for the decorator-tree
//!   integration arriving in Phase C (currently emits small badges at
//!   a list of `marker_xs`).

use std::borrow::Cow;

use crate::layer::{LayerId, LayerZ, SceneLayer};
use crate::paint::PaintContext;
use crate::primitives::{BadgeInstance, LineInstance, TextAnchor, TextInstance};

/// Long or short side. Only used by [`OrderBracketLayer`] to pick
/// leg colours.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

/// Minimal view struct for an order bracket — the full domain
/// `OrderBracket` type lives in `midas-app` and would pull a larger
/// dep graph. The scene layer only needs the prices and id.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderBracketView {
    pub id: u64,
    pub entry_price: f64,
    pub tp_price: Option<f64>,
    pub sl_price: Option<f64>,
    pub side: Side,
    pub label: Cow<'static, str>,
}

/// Horizontal legs + legend badges for each non-None bracket leg.
pub struct OrderBracketLayer {
    pub brackets: Vec<OrderBracketView>,
    pub line_width_px: f32,
}

impl OrderBracketLayer {
    pub fn new(brackets: Vec<OrderBracketView>) -> Self {
        Self {
            brackets,
            line_width_px: 1.5,
        }
    }
}

impl SceneLayer for OrderBracketLayer {
    fn id(&self) -> LayerId {
        LayerId("order-brackets")
    }

    fn z(&self) -> LayerZ {
        LayerZ::ORDER_BRACKET
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let w_px = ctx.viewport.width_px;
        for b in &self.brackets {
            let entry_color = match b.side {
                Side::Long => ctx.palette.candle_up,
                Side::Short => ctx.palette.candle_down,
            };
            let tp_color = ctx.palette.candle_up;
            let sl_color = ctx.palette.candle_down;

            let entry_y = ctx.price_to_y(b.entry_price);
            ctx.out.lines.push(LineInstance {
                x0: 0.0,
                y0: entry_y,
                x1: w_px,
                y1: entry_y,
                width_px: self.line_width_px,
                color: entry_color,
            });
            ctx.out.badges.push(BadgeInstance {
                x: w_px - 60.0,
                y: entry_y - 8.0,
                w: 56.0,
                h: 16.0,
                color: entry_color,
                text: b.label.clone(),
            });

            if let Some(tp) = b.tp_price {
                let y = ctx.price_to_y(tp);
                ctx.out.lines.push(LineInstance {
                    x0: 0.0,
                    y0: y,
                    x1: w_px,
                    y1: y,
                    width_px: self.line_width_px,
                    color: tp_color,
                });
            }
            if let Some(sl) = b.sl_price {
                let y = ctx.price_to_y(sl);
                ctx.out.lines.push(LineInstance {
                    x0: 0.0,
                    y0: y,
                    x1: w_px,
                    y1: y,
                    width_px: self.line_width_px,
                    color: sl_color,
                });
            }
        }
    }
}

/// Labelled horizontal price line.
#[derive(Clone, Debug, PartialEq)]
pub struct PriceLineView {
    pub id: u64,
    pub price: f64,
    pub label: Cow<'static, str>,
    pub color: [u8; 4],
}

/// User + order-bracket linked price lines. One `LineInstance` + one
/// right-edge `TextInstance` per entry.
pub struct PriceLineLayer {
    pub lines: Vec<PriceLineView>,
    pub line_width_px: f32,
}

impl PriceLineLayer {
    pub fn new(lines: Vec<PriceLineView>) -> Self {
        Self {
            lines,
            line_width_px: 1.0,
        }
    }
}

impl SceneLayer for PriceLineLayer {
    fn id(&self) -> LayerId {
        LayerId("price-lines")
    }

    fn z(&self) -> LayerZ {
        LayerZ::PRICE_LINE
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let w_px = ctx.viewport.width_px;
        for l in &self.lines {
            let y = ctx.price_to_y(l.price);
            ctx.out.lines.push(LineInstance {
                x0: 0.0,
                y0: y,
                x1: w_px,
                y1: y,
                width_px: self.line_width_px,
                color: l.color,
            });
            ctx.out.text.push(TextInstance {
                x: w_px - 4.0,
                y,
                color: ctx.palette.text,
                text: l.label.clone(),
                size_px: 11.0,
                anchor: TextAnchor::MiddleRight,
            });
        }
    }
}

/// Named price-level annotation.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelView {
    pub id: u64,
    pub price: f64,
    pub label: Cow<'static, str>,
    pub color: [u8; 4],
}

/// Named price levels — visually distinct from price lines by a
/// reduced alpha channel (the spec notes dashed/thicker in the GPU
/// renderer; MVP here differs only by alpha, keeping the primitive
/// vocabulary tight).
pub struct LevelLayer {
    pub levels: Vec<LevelView>,
    pub line_width_px: f32,
    /// Alpha scale applied to each level's colour to visually separate
    /// levels from price lines. `0.65` by default.
    pub alpha_scale: f32,
}

impl LevelLayer {
    pub fn new(levels: Vec<LevelView>) -> Self {
        Self {
            levels,
            line_width_px: 1.0,
            alpha_scale: 0.65,
        }
    }
}

impl SceneLayer for LevelLayer {
    fn id(&self) -> LayerId {
        LayerId("levels")
    }

    fn z(&self) -> LayerZ {
        LayerZ::LEVEL
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        let w_px = ctx.viewport.width_px;
        for lv in &self.levels {
            let y = ctx.price_to_y(lv.price);
            let mut color = lv.color;
            color[3] = ((color[3] as f32) * self.alpha_scale)
                .round()
                .clamp(0.0, 255.0) as u8;
            ctx.out.lines.push(LineInstance {
                x0: 0.0,
                y0: y,
                x1: w_px,
                y1: y,
                width_px: self.line_width_px,
                color,
            });
            ctx.out.text.push(TextInstance {
                x: 4.0,
                y,
                color: ctx.palette.text,
                text: lv.label.clone(),
                size_px: 11.0,
                anchor: TextAnchor::MiddleLeft,
            });
        }
    }
}

/// Placeholder for the Phase C decorator-tree integration. Renders a
/// small badge at each supplied pixel-x at the top of the viewport.
/// The final integration will consume a full `DecoratorTree` from
/// `midas-ui` — that's intentionally kept out of `midas-scene` so
/// this crate stays dep-light.
pub struct DecoratorLayer {
    pub marker_xs: Vec<f32>,
}

impl DecoratorLayer {
    pub fn new(marker_xs: Vec<f32>) -> Self {
        Self { marker_xs }
    }
}

impl SceneLayer for DecoratorLayer {
    fn id(&self) -> LayerId {
        LayerId("decorators")
    }

    fn z(&self) -> LayerZ {
        LayerZ::DECORATOR
    }

    fn paint(&self, ctx: &mut PaintContext<'_>) {
        for &x in &self.marker_xs {
            ctx.out.badges.push(BadgeInstance {
                x: x - 4.0,
                y: 4.0,
                w: 8.0,
                h: 8.0,
                color: ctx.palette.text,
                text: "".into(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};
    use midas_calendar::Timestamp;

    use super::*;
    use crate::primitives::ScenePrimitives;
    use crate::ThemePalette;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn harness() -> (
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
    fn order_bracket_with_all_legs_emits_three_lines() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = OrderBracketLayer::new(vec![OrderBracketView {
            id: 1,
            entry_price: 100.0,
            tp_price: Some(105.0),
            sl_price: Some(95.0),
            side: Side::Long,
            label: "E".into(),
        }]);
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 3);
        assert_eq!(out.badges.len(), 1);
    }

    #[test]
    fn order_bracket_entry_only_emits_one_line() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = OrderBracketLayer::new(vec![OrderBracketView {
            id: 2,
            entry_price: 100.0,
            tp_price: None,
            sl_price: None,
            side: Side::Short,
            label: "E".into(),
        }]);
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 1);
    }

    #[test]
    fn price_line_emits_one_line_per_entry() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = PriceLineLayer::new(vec![
            PriceLineView {
                id: 1,
                price: 100.0,
                label: "A".into(),
                color: [255, 0, 0, 255],
            },
            PriceLineView {
                id: 2,
                price: 105.0,
                label: "B".into(),
                color: [0, 255, 0, 255],
            },
        ]);
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.text.len(), 2);
    }

    #[test]
    fn level_layer_applies_alpha_scale() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        let layer = LevelLayer::new(vec![LevelView {
            id: 1,
            price: 100.0,
            label: "L".into(),
            color: [255, 255, 255, 200],
        }]);
        layer.paint(&mut ctx);
        assert_eq!(out.lines.len(), 1);
        // Default alpha_scale = 0.65 → 200*0.65 ≈ 130.
        assert!(out.lines[0].color[3] < 200);
    }

    #[test]
    fn decorator_layer_emits_one_badge_per_marker() {
        let (axis, paxis, pr, vp, pal, fmt) = harness();
        let mut out = ScenePrimitives::default();
        let mut ctx = PaintContext {
            axis: &axis,
            viewport: vp,
            price_range: pr,
            palette: &pal,
            price_axis: &paxis,
            formatter: &fmt,
            out: &mut out,
        };
        DecoratorLayer::new(vec![100.0, 200.0, 300.0]).paint(&mut ctx);
        assert_eq!(out.badges.len(), 3);
    }
}
