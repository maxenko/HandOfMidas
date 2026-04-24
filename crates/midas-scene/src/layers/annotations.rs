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
use std::sync::Arc;

use midas_axis::PriceRange;
use parking_lot::Mutex;

use crate::input::{CursorShape, EventStatus, Hit, InputEvent, Key, MouseButton, Point};
use crate::layer::{InteractiveLayer, LayerId, LayerZ, SceneLayer, ToolContext};
use crate::paint::PaintContext;
use crate::primitives::{BadgeInstance, LineInstance, TextAnchor, TextInstance};
use crate::tools::{ContextMenuAction, ContextMenuItem, ToolEffect};

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
    /// Whether the level is locked — drag + delete paths respect this
    /// flag. Added in slice 4 of the chart-transition plan; defaults
    /// to `false` for constructors that predate the addition.
    pub locked: bool,
}

/// In-flight drag state for a [`LevelLayer`].
///
/// Held in an `Arc<Mutex<_>>` so the layer can be rebuilt per frame
/// (the scene builder constructs fresh layer instances) while the drag
/// session persists across frames. The widget creates the Arc once
/// during widget init and hands one `Arc::clone` to every `LevelLayer`
/// it constructs afterwards.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LevelDragState {
    /// The id of the level currently being dragged. `None` while not
    /// dragging.
    pub dragging: Option<u64>,
}

/// Shared handle — the widget clones the `Arc` into each
/// [`LevelLayer::with_interaction`] call so drag state outlives the
/// per-frame layer instances.
pub type SharedLevelDrag = Arc<Mutex<LevelDragState>>;

/// Constants governing the level-layer hit-test geometry. Lifted from
/// plan slice 4 "Key implementation details".
const LEVEL_BAND_PX: f32 = 4.0;
const LOCK_ICON_SIZE_PX: f32 = 16.0;
const LOCK_ICON_OFFSET_PX: f32 = 24.0;
const DRAG_HANDLE_WIDTH_PX: f32 = 4.0;

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
    /// Shared drag state (slice 4). `None` for read-only / testing
    /// fixtures; construct with [`LevelLayer::with_interaction`] for
    /// the interactive path.
    drag: Option<SharedLevelDrag>,
    /// Viewport width remembered from the last paint. The hit-test path
    /// needs it to compute the lock-icon position (`width - 24`) and
    /// the drag-handle position (`width - DRAG_HANDLE_WIDTH_PX`).
    /// `paint` writes this via a `Mutex` to keep the `SceneLayer::paint`
    /// method signature `&self`.
    last_viewport_w: Arc<Mutex<f32>>,
    /// Viewport height last seen. Mirror of `last_viewport_w`.
    last_viewport_h: Arc<Mutex<f32>>,
}

impl LevelLayer {
    pub fn new(levels: Vec<LevelView>) -> Self {
        Self {
            levels,
            line_width_px: 1.0,
            alpha_scale: 0.65,
            drag: None,
            last_viewport_w: Arc::new(Mutex::new(0.0)),
            last_viewport_h: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Install a shared drag-state handle. Required for the
    /// interactive path (drag, hit-test).
    pub fn with_interaction(mut self, drag: SharedLevelDrag) -> Self {
        self.drag = Some(drag);
        self
    }

    /// Hit-test at `pt`. Returns `Some(LevelHit)` describing which
    /// level + target the cursor is over, or `None`.
    ///
    /// Priority (per slice 4):
    /// - Lock icon (16×16 at `width - LOCK_ICON_OFFSET_PX`) wins over
    ///   drag handle.
    /// - Drag handle (right `DRAG_HANDLE_WIDTH_PX` px) wins over the
    ///   line band.
    /// - Line band (±`LEVEL_BAND_PX` vertical) is the lowest.
    fn hit_level(&self, pt: Point, price_range: &PriceRange) -> Option<LevelHit> {
        let vp_w = *self.last_viewport_w.lock();
        let vp_h = *self.last_viewport_h.lock();
        if vp_h <= 0.0 {
            return None;
        }
        // Build an ephemeral PriceAxis so hit-test math matches paint.
        let paxis = midas_axis::LinearPriceAxis::new(*price_range, vp_h);
        for lv in &self.levels {
            let y = midas_axis::PriceAxis::to_y(&paxis, lv.price);
            if (pt.y - y).abs() > LEVEL_BAND_PX {
                continue;
            }
            // Lock icon: highest priority. Only present on locked
            // levels (a padlock glyph for unlocked could be added later
            // but is not in the MVP scope). We still report the target
            // for UNLOCKED levels so the widget can render a faint
            // icon-slot to discoverability — but we distinguish via
            // `.locked`.
            let lock_x0 = vp_w - LOCK_ICON_OFFSET_PX;
            let lock_x1 = lock_x0 + LOCK_ICON_SIZE_PX;
            let lock_y0 = y - LOCK_ICON_SIZE_PX / 2.0;
            let lock_y1 = y + LOCK_ICON_SIZE_PX / 2.0;
            if pt.x >= lock_x0 && pt.x <= lock_x1 && pt.y >= lock_y0 && pt.y <= lock_y1 {
                return Some(LevelHit {
                    level_id: lv.id,
                    target: LevelHitTarget::LockIcon,
                });
            }
            // Drag handle at right edge (but outside the lock-icon
            // band so they don't overlap).
            if pt.x >= vp_w - DRAG_HANDLE_WIDTH_PX && pt.x <= vp_w {
                return Some(LevelHit {
                    level_id: lv.id,
                    target: LevelHitTarget::DragHandle,
                });
            }
            // Line band — anywhere else within the vertical tolerance.
            return Some(LevelHit {
                level_id: lv.id,
                target: LevelHitTarget::LineBand,
            });
        }
        None
    }

    /// Compute the price under `pt.y` for a given price range (used by
    /// drag-move to translate the cursor position into a new price).
    fn y_to_price(&self, y: f32, price_range: &PriceRange) -> f64 {
        let vp_h = *self.last_viewport_h.lock();
        if vp_h <= 0.0 {
            return price_range.high();
        }
        let paxis = midas_axis::LinearPriceAxis::new(*price_range, vp_h);
        midas_axis::PriceAxis::from_y(&paxis, y).unwrap_or_else(|| price_range.high())
    }

    fn find_level(&self, id: u64) -> Option<&LevelView> {
        self.levels.iter().find(|l| l.id == id)
    }
}

/// Which visual affordance on a level the cursor is over.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LevelHitTarget {
    /// The line band (anywhere within ±4 px vertically).
    LineBand,
    /// The right-edge drag handle.
    DragHandle,
    /// The lock icon at `x = viewport.width - 24`.
    LockIcon,
}

/// One hit result from [`LevelLayer::hit_level`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LevelHit {
    pub level_id: u64,
    pub target: LevelHitTarget,
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
        // Remember viewport dims for the next hit-test call.
        *self.last_viewport_w.lock() = w_px;
        *self.last_viewport_h.lock() = ctx.viewport.height_px;
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
            if lv.locked {
                // Lock icon: a small dimmed badge at right edge.
                let icon_x = w_px - LOCK_ICON_OFFSET_PX;
                let icon_y = y - LOCK_ICON_SIZE_PX / 2.0;
                ctx.out.badges.push(BadgeInstance {
                    x: icon_x,
                    y: icon_y,
                    w: LOCK_ICON_SIZE_PX,
                    h: LOCK_ICON_SIZE_PX,
                    color: ctx.palette.text,
                    text: Cow::Borrowed("L"),
                });
            }
        }
    }

    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        if self.drag.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

impl InteractiveLayer for LevelLayer {
    fn update(&mut self, ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus {
        // Interactive path requires the shared drag handle.
        let Some(drag_state) = self.drag.as_ref() else {
            return EventStatus::Ignored;
        };
        match ev {
            InputEvent::MouseDown {
                button: MouseButton::Left,
                pt,
                ..
            } => {
                let Some(hit) = self.hit_level(pt, ctx.price_range) else {
                    return EventStatus::Ignored;
                };
                let level = match self.find_level(hit.level_id) {
                    Some(lv) => lv.clone(),
                    None => return EventStatus::Ignored,
                };
                if level.locked {
                    // Locked levels reject drag. Don't start a drag
                    // session; let the click fall through (e.g. for a
                    // future unlock-by-shift-click UX).
                    return EventStatus::Ignored;
                }
                // Only line-band + drag-handle hits start a drag.
                // Lock-icon on an unlocked level is not a drag gesture.
                if matches!(
                    hit.target,
                    LevelHitTarget::LineBand | LevelHitTarget::DragHandle
                ) {
                    drag_state.lock().dragging = Some(hit.level_id);
                    tracing::debug!(
                        target: "midas_scene::layers::annotations::level",
                        level_id = hit.level_id,
                        "LevelLayer began drag",
                    );
                    return EventStatus::Captured;
                }
                EventStatus::Ignored
            }
            InputEvent::MouseDown {
                button: MouseButton::Right,
                pt,
                ..
            } => {
                let Some(hit) = self.hit_level(pt, ctx.price_range) else {
                    return EventStatus::Ignored;
                };
                let level = match self.find_level(hit.level_id) {
                    Some(lv) => lv.clone(),
                    None => return EventStatus::Ignored,
                };
                let lock_label = if level.locked { "Unlock" } else { "Lock" };
                let items = vec![
                    ContextMenuItem {
                        label: "Edit".to_string(),
                        action: ContextMenuAction::Edit { id: level.id },
                    },
                    ContextMenuItem {
                        label: lock_label.to_string(),
                        action: ContextMenuAction::ToggleLock { id: level.id },
                    },
                    ContextMenuItem {
                        label: "Delete".to_string(),
                        action: ContextMenuAction::Delete { id: level.id },
                    },
                ];
                ctx.emit_effect(ToolEffect::OpenContextMenu { pt, items });
                EventStatus::Captured
            }
            InputEvent::MouseMove { pt } => {
                let dragging = { drag_state.lock().dragging };
                let Some(id) = dragging else {
                    return EventStatus::Ignored;
                };
                // Emit an UpdateLevel at the cursor's price. Caller
                // clamps / validates; the layer doesn't police the
                // value.
                let price = self.y_to_price(pt.y, ctx.price_range);
                ctx.emit_effect(ToolEffect::UpdateLevel { id, price });
                EventStatus::Captured
            }
            InputEvent::MouseUp { .. } => {
                let was_dragging = drag_state.lock().dragging.take();
                if was_dragging.is_some() {
                    tracing::debug!(
                        target: "midas_scene::layers::annotations::level",
                        "LevelLayer drag released",
                    );
                    EventStatus::Captured
                } else {
                    EventStatus::Ignored
                }
            }
            InputEvent::KeyDown {
                key: Key::Escape, ..
            } => {
                // Escape cancels an in-flight drag; we don't emit
                // UpdateLevel to restore the original price (the drag
                // already emitted `UpdateLevel`s continuously; the app
                // can implement "revert on escape" as a separate
                // bookkeeping layer).
                let was_dragging = drag_state.lock().dragging.take();
                if was_dragging.is_some() {
                    EventStatus::Captured
                } else {
                    EventStatus::Ignored
                }
            }
            _ => EventStatus::Ignored,
        }
    }

    fn hit_test(&self, pt: Point, price_range: &PriceRange) -> Option<Hit> {
        let hit = self.hit_level(pt, price_range)?;
        let cursor = match hit.target {
            LevelHitTarget::LineBand => CursorShape::ResizeNorthSouth,
            LevelHitTarget::DragHandle => CursorShape::Grab,
            LevelHitTarget::LockIcon => CursorShape::Pointer,
        };
        Some(Hit {
            layer_id: LayerId("levels"),
            sub_z: match hit.target {
                LevelHitTarget::LineBand => 0,
                LevelHitTarget::DragHandle => 1,
                LevelHitTarget::LockIcon => 2,
            },
            cursor,
        })
    }

    fn cancel(&mut self) {
        if let Some(drag) = &self.drag {
            drag.lock().dragging = None;
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
            locked: false,
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
