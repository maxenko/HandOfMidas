//! Pure layout algorithm for the decorator subsystem.
//!
//! Given a [`DecoratorGroup`] + a [`HoverState`], this module computes:
//!
//! - whether the group is visible on this frame ([`visibility_for`]);
//! - the sub-z band of every drawable item within the group
//!   ([`promote_by_proximity`]);
//! - an iterable emission list ready to be sorted by
//!   `(group_insertion_order, sub_z)` and painted
//!   ([`emissions_for_group`]).
//!
//! No tracing, no allocations beyond the returned `Vec` — tests
//! assert every axis without needing a rendering harness.

use crate::input::Point;
use crate::primitives::{BadgeInstance, LineInstance};

use super::{DecoratorGroup, DecoratorItem, HoverState, Visibility};

/// Proximity radius in CSS pixels. Within this distance the decorator
/// layer promotes an otherwise-background item into the proximity band
/// so it sits above its neighbours.
///
/// Legacy value is 20 px (`widget/compute/mod.rs` `PROXIMITY_PROMOTION_PX`)
/// but the chart-transition plan (slice 5a, key implementation details)
/// resets the threshold to 32 px for a comfortable target band.
pub const PROXIMITY_THRESHOLD_PX: f32 = 32.0;

/// Layer-wide alpha blend applied to every item inside a dragged
/// annotation's groups. Legacy uses `Presence::Ghost` → 0.5; we keep
/// the same factor for visual parity.
pub const DRAG_GHOST_ALPHA: f32 = 0.5;

/// Sub-z band within the decorator layer. Four values, matched to the
/// legacy four-pass renderer.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubZ {
    /// Default — nothing special happening to the owning item.
    Background = 0,
    /// Cursor is within [`PROXIMITY_THRESHOLD_PX`] of the group's
    /// parent bounds or any drawable item's bounds.
    Proximity = 1,
    /// Cursor is directly over the item's own rect.
    Hovered = 2,
    /// Owning annotation is in a drag session. Alpha-blended.
    Dragged = 3,
}

impl SubZ {
    /// The numeric ordinal. Used for painter-order sorting.
    #[inline]
    pub const fn ordinal(self) -> u8 {
        self as u8
    }
}

/// One decorator item with its computed sub-z assignment. The layer
/// takes a `Vec<PromotedItem>` per group, sorts them by `(sub_z,
/// insertion_idx)`, and emits.
#[derive(Clone, Debug, PartialEq)]
pub struct PromotedItem {
    /// Insertion index within the group's `items` — preserves stable
    /// tie-breaking when two items share a sub-z.
    pub index: usize,
    /// Which band the item paints into.
    pub sub_z: SubZ,
}

/// Evaluate whether `group` emits any items on this frame.
///
/// Rules (per plan slice 5a):
///
/// - `Visibility::Always` — always returns `true`. No further gating.
/// - `Visibility::OnHover { parent: None }` — requires the cursor to
///   be inside the group's own parent bounds, OR the group to be
///   sticky-expanded via [`HoverState::expanded_groups`], OR the group
///   owner annotation to be the currently-hovered annotation.
/// - `Visibility::OnHover { parent: Some(pid) }` — same, but the
///   hover-annotation check uses `pid` instead of
///   `group.annotation`. This lets a child group chain off a
///   different parent than its owning annotation.
///
/// Returning `false` means every item in the group is dropped for the
/// frame. Even drag-ghost alpha does not force emission of a hidden
/// group: if the user can't see it, dragging shouldn't reveal it.
pub fn visibility_for(group: &DecoratorGroup, hover: &HoverState) -> bool {
    match group.visibility {
        Visibility::Always => true,
        Visibility::OnHover { parent } => {
            let owner = parent.unwrap_or(group.annotation);
            // 1. Annotation hovered — parent chain check.
            if hover.is_annotation_hovered(owner) {
                return true;
            }
            // 2. Sticky-expanded group.
            if hover.is_group_expanded(group.id) {
                return true;
            }
            // 3. Cursor inside the parent bounds on this frame.
            if let Some(cursor) = hover.cursor_px {
                if group.parent_bounds.contains(cursor.x, cursor.y) {
                    return true;
                }
            }
            false
        }
    }
}

/// Compute sub-z per item for the group.
///
/// Promotion rules (applied in priority order; highest wins):
///
/// 1. `hover.dragged_annotation == Some(group.annotation)` → every
///    item lands in [`SubZ::Dragged`]. Alpha blend is applied at
///    emission time, not here.
/// 2. Item's own rect contains the cursor → [`SubZ::Hovered`].
/// 3. Item's rect is within [`PROXIMITY_THRESHOLD_PX`] of the cursor
///    *OR* the group's parent bounds are within the threshold →
///    [`SubZ::Proximity`].
/// 4. Otherwise → [`SubZ::Background`].
///
/// `cursor` is taken from [`HoverState::cursor_px`]; passing `None`
/// suppresses proximity + hover promotion (but dragging still wins).
///
/// Non-drawable items ([`DecoratorItem::Spacer`]) still appear in the
/// output so callers can round-trip through stable indices.
pub fn promote_by_proximity(
    items: &[DecoratorItem],
    cursor: Option<Point>,
    parent_bounds: crate::decorator::Rect,
    threshold_px: f32,
    dragged: bool,
) -> Vec<PromotedItem> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            if dragged {
                return PromotedItem {
                    index,
                    sub_z: SubZ::Dragged,
                };
            }
            let Some(pt) = cursor else {
                return PromotedItem {
                    index,
                    sub_z: SubZ::Background,
                };
            };
            // Hover — cursor directly over the item's own rect wins.
            let item_bounds = item.bounds();
            if item.is_drawable() && item_bounds.contains(pt.x, pt.y) {
                return PromotedItem {
                    index,
                    sub_z: SubZ::Hovered,
                };
            }
            // Proximity — either the parent bounds OR the item's own
            // rect is within `threshold_px` of the cursor. Legacy
            // walks each annotation's price-line distance; we widen
            // to the flat bounds so a Button sitting off to the side
            // of its parent annotation still promotes when the user's
            // finger hovers it.
            let parent_dist = parent_bounds.distance_to(pt.x, pt.y);
            let item_dist = if item.is_drawable() {
                item_bounds.distance_to(pt.x, pt.y)
            } else {
                f32::INFINITY
            };
            let nearest = parent_dist.min(item_dist);
            if nearest <= threshold_px {
                PromotedItem {
                    index,
                    sub_z: SubZ::Proximity,
                }
            } else {
                PromotedItem {
                    index,
                    sub_z: SubZ::Background,
                }
            }
        })
        .collect()
}

/// Apply [`DRAG_GHOST_ALPHA`] to the alpha channel of an RGBA8 colour.
/// Single-pass multiplication — the blend is layer-wide, not per-
/// item, so an item that is already semi-transparent stays
/// proportionally transparent.
#[inline]
pub fn apply_drag_ghost_alpha(color: [u8; 4]) -> [u8; 4] {
    let blended = (f32::from(color[3]) * DRAG_GHOST_ALPHA)
        .round()
        .clamp(0.0, 255.0) as u8;
    [color[0], color[1], color[2], blended]
}

/// One primitive emission produced by the decorator layer. Carries the
/// sub-z band so the layer can stable-sort across a collection of
/// groups before pushing into `ScenePrimitives`.
#[derive(Clone, Debug, PartialEq)]
pub enum DecoratorEmission {
    Line(LineInstance),
    Badge(BadgeInstance),
}

impl DecoratorEmission {
    /// Build an emission from a [`DecoratorItem`], applying drag-ghost
    /// alpha when `ghost` is `true`. Returns `None` for
    /// [`DecoratorItem::Spacer`] (nothing to draw).
    pub fn from_item(item: &DecoratorItem, ghost: bool) -> Option<Self> {
        match item {
            DecoratorItem::Line(l) => {
                let mut out = *l;
                if ghost {
                    out.color = apply_drag_ghost_alpha(out.color);
                }
                Some(DecoratorEmission::Line(out))
            }
            DecoratorItem::Badge(b) => {
                let mut out = b.clone();
                if ghost {
                    out.color = apply_drag_ghost_alpha(out.color);
                }
                Some(DecoratorEmission::Badge(out))
            }
            DecoratorItem::Button {
                bounds,
                color,
                label,
                ..
            } => {
                let out_color = if ghost {
                    apply_drag_ghost_alpha(*color)
                } else {
                    *color
                };
                Some(DecoratorEmission::Badge(BadgeInstance {
                    x: bounds.x0,
                    y: bounds.y0,
                    w: bounds.width(),
                    h: bounds.height(),
                    color: out_color,
                    text: label.clone(),
                }))
            }
            DecoratorItem::Spacer { .. } => None,
        }
    }
}

/// Walk one decorator group and return the list of `(sub_z, emission)`
/// entries it produces, in insertion order. Groups that fail visibility
/// return an empty vec.
///
/// The returned list is already alpha-blended (drag ghost) and carries
/// the sub-z band — the caller's job is to stable-sort by `(sub_z,
/// group_insertion_order, index)` and push into `ScenePrimitives`.
pub fn emissions_for_group(
    group: &DecoratorGroup,
    hover: &HoverState,
) -> Vec<(SubZ, usize, DecoratorEmission)> {
    if !visibility_for(group, hover) {
        return Vec::new();
    }
    let dragged = hover.is_annotation_dragged(group.annotation);
    let cursor = hover.cursor_px;
    let promoted = promote_by_proximity(
        &group.items,
        cursor,
        group.parent_bounds,
        PROXIMITY_THRESHOLD_PX,
        dragged,
    );
    promoted
        .into_iter()
        .filter_map(|p| {
            DecoratorEmission::from_item(&group.items[p.index], dragged)
                .map(|e| (p.sub_z, p.index, e))
        })
        .collect()
}
