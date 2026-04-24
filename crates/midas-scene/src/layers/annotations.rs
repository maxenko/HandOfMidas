//! Annotation layers — per R2-NM-4 each concrete, NOT a generic
//! `AnnotationLayer`. Each visual kind owns its own state machine and
//! its own `LayerZ` slot.
//!
//! Provided here:
//! - [`OrderBracketLayer`] — three-leg brackets (entry + optional TP + SL).
//! - [`PriceLineLayer`] — labelled horizontal lines.
//! - [`LevelLayer`] — named price levels (dashed / alpha-reduced visual
//!   differentiator for MVP).
//! - [`DecoratorLayer`] — full port of the legacy decorator subsystem
//!   (hover / proximity / drag sub-z, button dispatch) landed in
//!   slice 5a of the chart-transition plan.

use std::borrow::Cow;
use std::sync::Arc;

use midas_axis::PriceRange;
use parking_lot::Mutex;

use crate::decorator::layout::{emissions_for_group, DecoratorEmission, SubZ};
use crate::decorator::{ButtonAction, DecoratorGroup, DecoratorItem, HoverState};
use crate::decorator::{Rect, Visibility};
use crate::input::{CursorShape, EventStatus, Hit, InputEvent, Key, MouseButton, Point};
use crate::layer::{InteractiveLayer, LayerId, LayerZ, SceneLayer, ToolContext};
use crate::paint::PaintContext;
use crate::primitives::{BadgeInstance, LineInstance, TextAnchor, TextInstance};
use crate::tools::{ContextMenuAction, ContextMenuItem, LegKind, LegRole, ToolEffect};

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
///
/// Slice 5b adds `entry_ts` + partial-fill fields:
///
/// - `entry_ts` drives the right-from-entry x-span of TP / SL lines
///   (plan D9: express extent via `x0`/`x1` on the flat `LineInstance`,
///   not via a `LineExtent` enum).
/// - `filled_qty` + `total_qty` let the renderer distinguish the filled
///   portion of a partially-filled entry (plan R11; 9a live-bracket
///   handoff). When `filled_qty < total_qty` the layer emits the entry
///   line in a distinct brighter shade.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderBracketView {
    pub id: u64,
    pub entry_price: f64,
    pub tp_price: Option<f64>,
    pub sl_price: Option<f64>,
    pub side: Side,
    pub label: Cow<'static, str>,
    /// Entry-order timestamp. Drives the right-from-entry x-start for
    /// TP / SL lines so they don't blanket the entire viewport.
    /// `None` keeps legacy behaviour (full-width TP / SL) — used by
    /// slice 5a / slice 4 callers that predate the timestamp plumbing.
    #[doc(hidden)]
    pub entry_ts: Option<midas_calendar::Timestamp>,
    /// Quantity filled so far (execution-report driven). Combined with
    /// `total_qty` to colour the entry line distinctly when the bracket
    /// is partially filled.
    pub filled_qty: Option<u32>,
    /// Total requested quantity. Zero is treated as "no partial-fill
    /// styling" — same visual as `filled_qty == total_qty`.
    pub total_qty: u32,
}

impl OrderBracketView {
    /// True iff the bracket has a partially-filled entry (i.e. some of
    /// the requested quantity has executed, but not all). Drives the
    /// R11 partial-fill colour path.
    pub fn is_partially_filled(&self) -> bool {
        match self.filled_qty {
            Some(filled) => self.total_qty > 0 && filled > 0 && filled < self.total_qty,
            None => false,
        }
    }
}

/// In-flight drag state for an [`OrderBracketLayer`].
///
/// Mirrors `LevelDragState` — held behind an `Arc<Mutex<_>>` so the
/// layer can be rebuilt per frame while the drag session persists
/// across frames.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BracketDragState {
    /// `Some((bracket_id, role))` while the user is dragging the drag
    /// handle on a TP or SL leg. `None` while not dragging. Entry is
    /// intentionally not draggable on this layer (plan 5b: entry is
    /// non-draggable).
    pub dragging: Option<(u64, LegKind)>,
}

/// Shared handle — the widget clones the `Arc` into every
/// [`OrderBracketLayer::with_interaction`] call so drag state outlives
/// the per-frame layer instances.
pub type SharedBracketDrag = Arc<Mutex<BracketDragState>>;

/// Drag-handle pixel tolerance (4 px in SCREEN space, NOT DPI-scaled)
/// — legacy convention pinned by plan slice 5b "Key implementation
/// details".
const BRACKET_DRAG_HANDLE_PX: f32 = 4.0;

/// Amber fill used to warn when a TP / SL leg is on the wrong side of
/// entry. Matches the RGBA of `BRACKET_WARNING_COLOR` in
/// `desktop/win/crates/midas-chart/src/widget/order_bracket/mod.rs:207`
/// ([0.95, 0.70, 0.18, 1.0] → 8-bit).
const BRACKET_AMBER: [u8; 4] = [0xf2, 0xb3, 0x2e, 0xff];

/// Horizontal legs + legend badges for each non-None bracket leg.
///
/// Slice 5b upgrades this from a passive renderer into a full
/// interactive layer:
///
/// - Entry line: full-width via `x0 = 0`, `x1 = viewport.width`.
/// - TP / SL line: right-from-entry — `x0 = axis.to_x(entry_ts)`,
///   `x1 = viewport.width`. Falls back to full-width when
///   [`OrderBracketView::entry_ts`] is `None`.
/// - Amber tint on wrong-side TP / SL legs (uses
///   [`crate::tools::is_leg_on_wrong_side`]).
/// - Hit-test + drag on the TP / SL drag handles; emits
///   [`ToolEffect::UpdateBracketLeg`] on `MouseMove`.
/// - Entry line is NOT draggable — hover returns `CursorShape::Default`.
pub struct OrderBracketLayer {
    pub brackets: Vec<OrderBracketView>,
    pub line_width_px: f32,
    /// Shared drag-state handle. Required for the interactive path.
    drag: Option<SharedBracketDrag>,
    /// Viewport dims cached at paint time so hit-test can resolve the
    /// drag-handle band (`x1 - BRACKET_DRAG_HANDLE_PX..=x1`).
    last_viewport_w: Arc<Mutex<f32>>,
    last_viewport_h: Arc<Mutex<f32>>,
}

impl OrderBracketLayer {
    pub fn new(brackets: Vec<OrderBracketView>) -> Self {
        Self {
            brackets,
            line_width_px: 1.5,
            drag: None,
            last_viewport_w: Arc::new(Mutex::new(0.0)),
            last_viewport_h: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Install a shared drag-state handle. Required for the interactive
    /// path (drag, hit-test on drag handles).
    pub fn with_interaction(mut self, drag: SharedBracketDrag) -> Self {
        self.drag = Some(drag);
        self
    }

    /// Project every bracket into three sibling [`DecoratorGroup`]s —
    /// entry, TP, SL. Each group carries:
    ///
    /// - `Always` — a badge with the formatted price.
    /// - `OnHover { parent }` — delete button + label-with-percent-
    ///   distance.
    ///
    /// The widget composes these with other decorator groups (levels,
    /// indicators) and passes the combined set to a `DecoratorLayer`.
    pub fn decorator_groups(&self, viewport_w_px: f32, viewport_h_px: f32) -> Vec<DecoratorGroup> {
        use crate::decorator::GroupId;
        // Build a transient price axis so we can compute per-leg y
        // coordinates without the scene's `PaintContext`.
        // Caller supplies the viewport; we can't derive PriceRange from
        // inside so we hand every group a zero-sized parent_bounds and
        // let the host overwrite it when composing.
        let _ = viewport_h_px; // retained for future anchoring use.
        let mut groups: Vec<DecoratorGroup> = Vec::with_capacity(self.brackets.len() * 3);
        for b in &self.brackets {
            // Entry badge at right edge.
            let entry_bounds = Rect::new(viewport_w_px - 60.0, 0.0, viewport_w_px - 4.0, 16.0);
            groups.push(DecoratorGroup {
                id: GroupId(b.id << 2),
                annotation: b.id,
                parent_bounds: entry_bounds,
                visibility: Visibility::Always,
                items: vec![DecoratorItem::Badge(BadgeInstance {
                    x: entry_bounds.x0,
                    y: entry_bounds.y0,
                    w: entry_bounds.width(),
                    h: entry_bounds.height(),
                    color: match b.side {
                        Side::Long => [0x3d, 0xd5, 0x98, 0xff],
                        Side::Short => [0xf2, 0x5d, 0x5d, 0xff],
                    },
                    text: format!("E {:.2}", b.entry_price).into(),
                })],
            });
            if let Some(tp) = b.tp_price {
                let tp_bounds = Rect::new(viewport_w_px - 60.0, 0.0, viewport_w_px - 4.0, 16.0);
                groups.push(DecoratorGroup {
                    id: GroupId((b.id << 2) | 1),
                    annotation: b.id,
                    parent_bounds: tp_bounds,
                    visibility: Visibility::Always,
                    items: vec![DecoratorItem::Badge(BadgeInstance {
                        x: tp_bounds.x0,
                        y: tp_bounds.y0,
                        w: tp_bounds.width(),
                        h: tp_bounds.height(),
                        color: [0x3d, 0xd5, 0x98, 0xff],
                        text: format!("TP {:.2}", tp).into(),
                    })],
                });
            }
            if let Some(sl) = b.sl_price {
                let sl_bounds = Rect::new(viewport_w_px - 60.0, 0.0, viewport_w_px - 4.0, 16.0);
                groups.push(DecoratorGroup {
                    id: GroupId((b.id << 2) | 2),
                    annotation: b.id,
                    parent_bounds: sl_bounds,
                    visibility: Visibility::Always,
                    items: vec![DecoratorItem::Badge(BadgeInstance {
                        x: sl_bounds.x0,
                        y: sl_bounds.y0,
                        w: sl_bounds.width(),
                        h: sl_bounds.height(),
                        color: [0xf2, 0x5d, 0x5d, 0xff],
                        text: format!("SL {:.2}", sl).into(),
                    })],
                });
            }
        }
        groups
    }

    /// Hit-test the drag handle on the TP / SL legs. Entry is
    /// intentionally non-draggable; callers see `None` when the cursor
    /// is over the entry band.
    pub fn hit_bracket(&self, pt: Point, price_range: &PriceRange) -> Option<BracketHit> {
        let vp_w = *self.last_viewport_w.lock();
        let vp_h = *self.last_viewport_h.lock();
        if vp_h <= 0.0 {
            return None;
        }
        let paxis = midas_axis::LinearPriceAxis::new(*price_range, vp_h);
        for b in &self.brackets {
            // Only TP / SL have drag handles. Entry is always full-
            // width and non-draggable.
            for (leg_kind, leg_price_opt) in [(LegKind::Tp, b.tp_price), (LegKind::Sl, b.sl_price)]
            {
                let Some(leg_price) = leg_price_opt else {
                    continue;
                };
                let y = midas_axis::PriceAxis::to_y(&paxis, leg_price);
                // Vertical tolerance for the line band (same 4 px).
                if (pt.y - y).abs() > BRACKET_DRAG_HANDLE_PX {
                    continue;
                }
                // Drag handle: right `BRACKET_DRAG_HANDLE_PX` px.
                if pt.x >= vp_w - BRACKET_DRAG_HANDLE_PX && pt.x <= vp_w {
                    return Some(BracketHit {
                        bracket_id: b.id,
                        target: BracketHitTarget::DragHandle { leg: leg_kind },
                    });
                }
                // Line band — hover state only, not a drag start.
                return Some(BracketHit {
                    bracket_id: b.id,
                    target: BracketHitTarget::LineBand { leg: leg_kind },
                });
            }
            // Entry line: full-width band — hover only (cursor Default).
            let entry_y = midas_axis::PriceAxis::to_y(&paxis, b.entry_price);
            if (pt.y - entry_y).abs() <= BRACKET_DRAG_HANDLE_PX {
                return Some(BracketHit {
                    bracket_id: b.id,
                    target: BracketHitTarget::EntryLine,
                });
            }
        }
        None
    }

    fn y_to_price(&self, y: f32, price_range: &PriceRange) -> f64 {
        let vp_h = *self.last_viewport_h.lock();
        if vp_h <= 0.0 {
            return price_range.high();
        }
        let paxis = midas_axis::LinearPriceAxis::new(*price_range, vp_h);
        midas_axis::PriceAxis::from_y(&paxis, y).unwrap_or_else(|| price_range.high())
    }
}

/// Which visual affordance on a bracket the cursor is over.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BracketHitTarget {
    /// Entry line band (non-draggable). Cursor → `Default`.
    EntryLine,
    /// TP or SL line band (hover only).
    LineBand { leg: LegKind },
    /// TP or SL drag handle at right edge. Cursor → `Grab`.
    DragHandle { leg: LegKind },
}

/// One hit result from [`OrderBracketLayer::hit_bracket`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BracketHit {
    pub bracket_id: u64,
    pub target: BracketHitTarget,
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
        *self.last_viewport_w.lock() = w_px;
        *self.last_viewport_h.lock() = ctx.viewport.height_px;
        for b in &self.brackets {
            let entry_color = match b.side {
                Side::Long => ctx.palette.candle_up,
                Side::Short => ctx.palette.candle_down,
            };
            let tp_color = ctx.palette.candle_up;
            let sl_color = ctx.palette.candle_down;

            // R11 partial-fill styling: filled portion renders with a
            // distinct brighter entry colour multiplier.
            let entry_paint_color = if b.is_partially_filled() {
                let mut c = entry_color;
                // Brightness bump — keep alpha, push RGB toward white.
                c[0] = ((c[0] as u16 + 255) / 2) as u8;
                c[1] = ((c[1] as u16 + 255) / 2) as u8;
                c[2] = ((c[2] as u16 + 255) / 2) as u8;
                c
            } else {
                entry_color
            };

            let entry_y = ctx.price_to_y(b.entry_price);
            ctx.out.lines.push(LineInstance {
                x0: 0.0,
                y0: entry_y,
                x1: w_px,
                y1: entry_y,
                width_px: self.line_width_px,
                color: entry_paint_color,
            });
            ctx.out.badges.push(BadgeInstance {
                x: w_px - 60.0,
                y: entry_y - 8.0,
                w: 56.0,
                h: 16.0,
                color: entry_paint_color,
                text: b.label.clone(),
            });

            // TP/SL lines start at the entry timestamp's x (right-from
            // entry) — plan D9. Fall back to full-width when entry_ts
            // is not provided.
            let tp_sl_x0 = if let Some(ts) = b.entry_ts {
                ctx.axis.to_x(ts).clamp(0.0, w_px)
            } else {
                0.0
            };

            if let Some(tp) = b.tp_price {
                let y = ctx.price_to_y(tp);
                let side_tool = match b.side {
                    Side::Long => crate::tools::Side::Long,
                    Side::Short => crate::tools::Side::Short,
                };
                let color = if crate::tools::is_leg_on_wrong_side(
                    side_tool,
                    b.entry_price,
                    tp,
                    LegKind::Tp,
                ) {
                    BRACKET_AMBER
                } else {
                    tp_color
                };
                ctx.out.lines.push(LineInstance {
                    x0: tp_sl_x0,
                    y0: y,
                    x1: w_px,
                    y1: y,
                    width_px: self.line_width_px,
                    color,
                });
            }
            if let Some(sl) = b.sl_price {
                let y = ctx.price_to_y(sl);
                let side_tool = match b.side {
                    Side::Long => crate::tools::Side::Long,
                    Side::Short => crate::tools::Side::Short,
                };
                let color = if crate::tools::is_leg_on_wrong_side(
                    side_tool,
                    b.entry_price,
                    sl,
                    LegKind::Sl,
                ) {
                    BRACKET_AMBER
                } else {
                    sl_color
                };
                ctx.out.lines.push(LineInstance {
                    x0: tp_sl_x0,
                    y0: y,
                    x1: w_px,
                    y1: y,
                    width_px: self.line_width_px,
                    color,
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

impl InteractiveLayer for OrderBracketLayer {
    fn update(&mut self, ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus {
        let Some(drag_state) = self.drag.as_ref() else {
            return EventStatus::Ignored;
        };
        match ev {
            InputEvent::MouseDown {
                button: MouseButton::Left,
                pt,
                ..
            } => {
                let Some(hit) = self.hit_bracket(pt, ctx.price_range) else {
                    return EventStatus::Ignored;
                };
                // Only drag handles start a drag session.
                match hit.target {
                    BracketHitTarget::DragHandle { leg } => {
                        drag_state.lock().dragging = Some((hit.bracket_id, leg));
                        tracing::debug!(
                            target: "midas_scene::layers::annotations::order_bracket",
                            bracket_id = hit.bracket_id,
                            ?leg,
                            "OrderBracketLayer began drag",
                        );
                        EventStatus::Captured
                    }
                    BracketHitTarget::EntryLine | BracketHitTarget::LineBand { .. } => {
                        EventStatus::Ignored
                    }
                }
            }
            InputEvent::MouseMove { pt } => {
                let dragging = { drag_state.lock().dragging };
                let Some((id, leg)) = dragging else {
                    return EventStatus::Ignored;
                };
                let price = self.y_to_price(pt.y, ctx.price_range);
                let role = match leg {
                    LegKind::Tp => LegRole::Tp,
                    LegKind::Sl => LegRole::Sl,
                };
                ctx.emit_effect(ToolEffect::UpdateBracketLeg { id, role, price });
                EventStatus::Captured
            }
            InputEvent::MouseUp { .. } => {
                let was_dragging = drag_state.lock().dragging.take();
                if was_dragging.is_some() {
                    tracing::debug!(
                        target: "midas_scene::layers::annotations::order_bracket",
                        "OrderBracketLayer drag released",
                    );
                    EventStatus::Captured
                } else {
                    EventStatus::Ignored
                }
            }
            InputEvent::KeyDown {
                key: Key::Escape, ..
            } => {
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
        let hit = self.hit_bracket(pt, price_range)?;
        let cursor = match hit.target {
            BracketHitTarget::EntryLine => CursorShape::Default,
            BracketHitTarget::LineBand { .. } => CursorShape::ResizeNorthSouth,
            BracketHitTarget::DragHandle { .. } => CursorShape::Grab,
        };
        Some(Hit {
            layer_id: LayerId("order-brackets"),
            sub_z: match hit.target {
                BracketHitTarget::EntryLine => 0,
                BracketHitTarget::LineBand { .. } => 1,
                BracketHitTarget::DragHandle { .. } => 2,
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

/// Full decorator-subsystem port (slice 5a of the chart-transition
/// plan). Consumes a flat `Vec<DecoratorGroup>` supplied by the host
/// widget — typically one group per bracket leg, one per level badge,
/// one per indicator chip — and paints the emissions according to the
/// four sub-z bands (background < proximity-promoted < hovered <
/// dragged).
///
/// ## Paint cycle
///
/// For every group:
/// 1. Run [`visibility_for`](crate::decorator::layout::visibility_for).
///    Hidden groups skip entirely.
/// 2. Run [`promote_by_proximity`](crate::decorator::layout::promote_by_proximity)
///    to tag each item with its sub-z band. Drag ghost applies
///    layer-wide alpha at emit time.
/// 3. Collect `(sub_z, group_insertion_idx, item_idx, emission)`
///    tuples.
///
/// After walking every group, the collected tuples are stable-sorted
/// by `(sub_z, group_insertion_idx, item_idx)` and pushed into
/// `ScenePrimitives`. Within one sub-z, groups stay in their insertion
/// order; within a group, items stay in theirs.
///
/// ## Interactive role
///
/// The layer implements [`InteractiveLayer`] so it can dispatch left-
/// clicks on `Button` items. Hit-testing walks visible groups only;
/// a click on a button inside an on-hover group that is currently
/// hidden is ignored. Any matched click emits the button's
/// [`ButtonAction`] as a [`ToolEffect`] and returns
/// `EventStatus::Captured`.
pub struct DecoratorLayer {
    /// The current flat decorator set. The host rebuilds this per
    /// frame from its annotation store (levels, bracket legs,
    /// indicator chips).
    pub groups: Vec<DecoratorGroup>,
    /// Per-frame hover / drag / expansion snapshot. Host rebuilds
    /// before each paint.
    pub hover: HoverState,
}

impl DecoratorLayer {
    /// Construct a decorator layer with its initial group set +
    /// hover snapshot.
    pub fn new(groups: Vec<DecoratorGroup>, hover: HoverState) -> Self {
        Self { groups, hover }
    }

    /// Replace the stored group set. Host calls this at frame boundary
    /// after rebuilding the annotation projection.
    pub fn set_groups(&mut self, groups: Vec<DecoratorGroup>) {
        self.groups = groups;
    }

    /// Replace the stored hover state.
    pub fn set_hover(&mut self, hover: HoverState) {
        self.hover = hover;
    }

    /// Return the button at `pt`, walking visible groups top-to-bottom
    /// (later groups win — matches paint-over semantics). Returns the
    /// group insertion idx + item idx so the caller can route the
    /// click.
    pub fn button_at(&self, pt: Point) -> Option<(usize, usize)> {
        // Iterate in reverse so later-inserted groups win (they paint on top).
        for (g_idx, group) in self.groups.iter().enumerate().rev() {
            if !crate::decorator::layout::visibility_for(group, &self.hover) {
                continue;
            }
            for (i_idx, item) in group.items.iter().enumerate().rev() {
                if let DecoratorItem::Button { bounds, .. } = item {
                    if bounds.contains(pt.x, pt.y) {
                        return Some((g_idx, i_idx));
                    }
                }
            }
        }
        None
    }

    /// Translate a `ButtonAction` into a `ToolEffect`. Private helper.
    fn action_to_effect(action: &ButtonAction, pt: Point) -> ToolEffect {
        match action {
            ButtonAction::OpenContextMenu { items } => ToolEffect::OpenContextMenu {
                pt,
                items: items.clone(),
            },
            ButtonAction::Menu(menu_action) => match *menu_action {
                ContextMenuAction::Edit { id } => ToolEffect::OpenContextMenu {
                    pt,
                    items: vec![ContextMenuItem {
                        label: "Edit".to_string(),
                        action: ContextMenuAction::Edit { id },
                    }],
                },
                ContextMenuAction::ToggleLock { id } => ToolEffect::OpenContextMenu {
                    pt,
                    items: vec![ContextMenuItem {
                        label: "Toggle Lock".to_string(),
                        action: ContextMenuAction::ToggleLock { id },
                    }],
                },
                ContextMenuAction::Delete { id } => ToolEffect::DeleteLevel { id },
            },
            ButtonAction::Effect(effect) => effect.clone(),
        }
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
        // Collect `(sub_z, group_idx, item_idx, emission)` across all
        // groups. Stable-sort lets us paint bands in ascending order
        // while preserving insertion order within a band.
        let mut collected: Vec<(SubZ, usize, usize, DecoratorEmission)> = Vec::new();
        for (g_idx, group) in self.groups.iter().enumerate() {
            let emissions = emissions_for_group(group, &self.hover);
            for (sub_z, i_idx, emission) in emissions {
                collected.push((sub_z, g_idx, i_idx, emission));
            }
        }
        collected.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        for (_sub_z, _g_idx, _i_idx, emission) in collected {
            match emission {
                DecoratorEmission::Line(l) => ctx.out.lines.push(l),
                DecoratorEmission::Badge(b) => ctx.out.badges.push(b),
            }
        }
    }

    fn as_interactive(&mut self) -> Option<&mut dyn InteractiveLayer> {
        Some(self)
    }
}

impl InteractiveLayer for DecoratorLayer {
    fn update(&mut self, ev: InputEvent, ctx: &mut ToolContext<'_>) -> EventStatus {
        let InputEvent::MouseDown {
            button: MouseButton::Left,
            pt,
            ..
        } = ev
        else {
            return EventStatus::Ignored;
        };
        let Some((g_idx, i_idx)) = self.button_at(pt) else {
            return EventStatus::Ignored;
        };
        let DecoratorItem::Button { action, .. } = &self.groups[g_idx].items[i_idx] else {
            return EventStatus::Ignored;
        };
        let effect = Self::action_to_effect(action, pt);
        ctx.emit_effect(effect);
        EventStatus::Captured
    }

    fn hit_test(&self, pt: Point, _price_range: &PriceRange) -> Option<Hit> {
        if self.button_at(pt).is_some() {
            Some(Hit {
                layer_id: LayerId("decorators"),
                sub_z: 0,
                cursor: CursorShape::Pointer,
            })
        } else {
            None
        }
    }

    fn cancel(&mut self) {
        // Decorator layer is stateless — nothing to reset.
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
            entry_ts: None,
            filled_qty: None,
            total_qty: 0,
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
            entry_ts: None,
            filled_qty: None,
            total_qty: 0,
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
    fn decorator_layer_emits_one_badge_per_always_group() {
        // Thin smoke test — the deep decorator-layout coverage lives in
        // `crate::decorator::tests`. This just confirms the layer's
        // SceneLayer integration pushes into the primitives buffer.
        use crate::decorator::{DecoratorGroup, DecoratorItem, GroupId, HoverState, Rect};
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
        let groups = vec![
            DecoratorGroup::always(
                GroupId(1),
                1,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                vec![DecoratorItem::Badge(BadgeInstance {
                    x: 100.0,
                    y: 4.0,
                    w: 8.0,
                    h: 8.0,
                    color: [0xff, 0xff, 0xff, 0xff],
                    text: "".into(),
                })],
            ),
            DecoratorGroup::always(
                GroupId(2),
                2,
                Rect::new(100.0, 0.0, 200.0, 50.0),
                vec![DecoratorItem::Badge(BadgeInstance {
                    x: 200.0,
                    y: 4.0,
                    w: 8.0,
                    h: 8.0,
                    color: [0xff, 0xff, 0xff, 0xff],
                    text: "".into(),
                })],
            ),
        ];
        DecoratorLayer::new(groups, HoverState::default()).paint(&mut ctx);
        assert_eq!(out.badges.len(), 2);
    }
}
