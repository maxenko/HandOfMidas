//! Decorator compute engine: walks a `DecoratorGroup` anchored to a `PriceLine`
//! and produces a `WidgetOutput` of fills, badges, labels, and hit zones.

use super::badge::{Badge, BadgeBorder, BadgeSegment, BadgeShape};
use super::button::Button;
use super::group::{
    DecoratorAnchor, DecoratorGroup, DecoratorItem, FlexDirection, ItemContent, Visibility,
};
use super::layout::{measure_badge, measure_item, measure_text};
use crate::instances::{BadgeInstance, GridLineInstance};
use crate::widget::compute::{ComputeContext, LabelAnchor, WidgetLabel, WidgetOutput};
use crate::widget::hit_test::{CursorIcon, HitZone, HitZoneKind, ItemPath};
use crate::widget::price_line::PriceLine;
use crate::widget::AnnotationId;

/// A single decorator group rooted at a parent `PriceLine` for which
/// hit zones should be recomputed. Used by
/// [`recompute_decorator_hit_zones`] to let the app-layer `update()`
/// loop ask "is the cursor currently over any visible item in this
/// group?" without having to reach into the full widget-output cache.
pub struct DecoratorGroupRef<'a> {
    /// Parent annotation id (same id used for [`HitZone::annotation_id`]).
    pub annotation_id: AnnotationId,
    /// The group itself.
    pub group: &'a DecoratorGroup,
    /// The price line the group is anchored to. Only `price` is read.
    pub line: &'a PriceLine,
}

/// Re-run `compute_decorator_group` for a slice of `(annotation_id,
/// group, line)` triples and return the flat list of hit zones.
///
/// Called from `chart_widget.rs::update()` on every mouse-move event so
/// the hover set machinery has a fresh picture of where every
/// hover-revealed button lives *for the current frame's cursor*. This
/// sidesteps the cache-invalidation problem described in
/// `plan/decorator-system/05-interaction.md` — "Why recompute instead
/// of cache".
///
/// The function is pure: it calls `compute_decorator_group` with an
/// `alpha` of 1.0, strips the rendering primitives, and returns only
/// the `HitZone` list. Non-`Decorator` zones are preserved in the
/// output (defensive — the current implementation only emits
/// `HitZoneKind::Decorator` from the decorator pipeline, but callers
/// should not assume).
pub fn recompute_decorator_hit_zones(
    groups: &[DecoratorGroupRef<'_>],
    ctx: &ComputeContext<'_>,
) -> Vec<HitZone> {
    let mut out = Vec::new();
    for g in groups {
        let widget = compute_decorator_group(g.group, g.line, g.annotation_id, ctx, 1.0);
        out.extend(widget.hit_zones);
    }
    out
}

/// Test whether a screen-space point is inside a hit-zone rect.
///
/// Inclusive on all four edges so a cursor that lands exactly on the
/// item's border still counts as over it — matches the rest of the
/// interaction layer's hit-test semantics.
pub fn rect_contains(rect: [f32; 4], x: f32, y: f32) -> bool {
    x >= rect[0] && x <= rect[2] && y >= rect[1] && y <= rect[3]
}

/// Compute render primitives for a decorator group anchored to a [`PriceLine`].
///
/// Fill / label / stroke alpha is multiplied by the incoming `alpha`
/// parameter so that `Presence::Ghost` chains cleanly through the
/// decorator tree. Pure: reads `ctx`, allocates a fresh `WidgetOutput`,
/// and returns it.
pub fn compute_decorator_group(
    group: &DecoratorGroup,
    line: &PriceLine,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
) -> WidgetOutput {
    let anchor_x = resolve_anchor_x(&group.anchor, ctx);
    let anchor_y = ctx.camera.price_to_y(line.price);
    compute_decorator_group_at(group, anchor_x, anchor_y, annotation_id, ctx, alpha, &[])
}

/// Recursive worker. `path_prefix` is prepended to every hit zone's
/// `item_path` so nested `Stack` children inherit their parent item's
/// breadcrumb.
fn compute_decorator_group_at(
    group: &DecoratorGroup,
    anchor_x: f32,
    anchor_y: f32,
    annotation_id: AnnotationId,
    ctx: &ComputeContext<'_>,
    alpha: f32,
    path_prefix: &[u8],
) -> WidgetOutput {
    let mut output = WidgetOutput::empty();

    // Gate `OnLineHover` / `OnGroupHover` against the cursor state
    // carried on `ComputeContext`. See 05-interaction.md for the
    // canonical rules.
    let line_hovered = ctx
        .hovered_annotation
        .map(|(aid, _)| aid == annotation_id)
        .unwrap_or(false);
    let group_expanded = ctx
        .hovered_decorator_groups
        .iter()
        .any(|&(aid, gid)| aid == annotation_id && gid == group.group_id);

    // Pass 1: visibility filter + measurement. Original indices are
    // preserved because `ItemPath` encodes them. Skipped items contribute
    // zero footprint — the group's anchor stays fixed and the layout
    // expands/contracts from there ("drawer slides open" feel).
    let visible: Vec<(usize, &DecoratorItem, (f32, f32))> = group
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| match item.visibility {
            Visibility::Always => true,
            Visibility::OnLineHover => line_hovered,
            Visibility::OnGroupHover => line_hovered || group_expanded,
        })
        .map(|(idx, item)| (idx, item, measure_item(item)))
        .collect();

    if visible.is_empty() {
        return output;
    }

    // Pass 2: positioning along the main axis, centered on the cross
    // axis against the anchor.
    //
    // `Row + RightEdge` packs right-to-left (items accumulate leftward
    // from the viewport edge). Every other configuration packs forward
    // from the anchor along the main axis.
    let row_right_to_left = matches!(group.direction, FlexDirection::Row)
        && matches!(group.anchor, DecoratorAnchor::RightEdge);

    let mut cursor = match group.direction {
        FlexDirection::Row => anchor_x,
        FlexDirection::Column => anchor_y,
    };

    for (pass_idx, (item_idx, item, (w, h))) in visible.iter().enumerate() {
        let w = *w;
        let h = *h;
        let (x0, y0) = match group.direction {
            FlexDirection::Row => {
                let x0 = if row_right_to_left {
                    if pass_idx > 0 {
                        cursor -= group.gap;
                    }
                    cursor -= w;
                    cursor
                } else {
                    if pass_idx > 0 {
                        cursor += group.gap;
                    }
                    let x = cursor;
                    cursor += w;
                    x
                };
                (x0, anchor_y - h / 2.0)
            }
            FlexDirection::Column => {
                if pass_idx > 0 {
                    cursor += group.gap;
                }
                let y = cursor;
                cursor += h;
                (anchor_x - w / 2.0, y)
            }
        };
        let rect = [x0, y0, x0 + w, y0 + h];

        emit_item(
            *item_idx,
            item,
            rect,
            group.group_id,
            annotation_id,
            alpha,
            path_prefix,
            ctx,
            &mut output,
        );
    }

    output
}

fn resolve_anchor_x(anchor: &DecoratorAnchor, ctx: &ComputeContext<'_>) -> f32 {
    match *anchor {
        DecoratorAnchor::LeftEdge => 0.0,
        DecoratorAnchor::RightEdge => ctx.viewport.width as f32,
        DecoratorAnchor::AtTimestamp(t) => ctx.camera.time_to_x(t as f64),
        DecoratorAnchor::AtScreenX(x) => x,
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_item(
    item_idx: usize,
    item: &DecoratorItem,
    rect: [f32; 4],
    group_id: u16,
    annotation_id: AnnotationId,
    alpha: f32,
    path_prefix: &[u8],
    ctx: &ComputeContext<'_>,
    output: &mut WidgetOutput,
) {
    match &item.content {
        ItemContent::Badge(badge) => {
            emit_badge(
                item_idx,
                badge,
                item.action,
                rect,
                group_id,
                annotation_id,
                alpha,
                path_prefix,
                output,
            );
        }
        ItemContent::Button(button) => {
            emit_button_shape(button, rect, alpha, output);
            // One label centered on the button for the glyph.
            let mut buf = [0u8; 4];
            let glyph_text = button.glyph.encode_utf8(&mut buf).to_string();
            let label_color = apply_alpha(button.glyph_color, alpha);
            output.labels.push(WidgetLabel {
                text: glyph_text,
                screen_x: (rect[0] + rect[2]) * 0.5,
                screen_y: (rect[1] + rect[3]) * 0.5,
                bg_color: [0.0; 4],
                text_color: label_color,
                font_size: button.glyph_size,
                anchor: LabelAnchor::Center,
            });

            if let Some(action) = item.action {
                output.hit_zones.push(HitZone {
                    annotation_id,
                    rect,
                    kind: HitZoneKind::Decorator {
                        group_id,
                        item_path: build_path(path_prefix, &[item_idx as u8]),
                        action,
                    },
                    cursor: CursorIcon::Pointer,
                });
            }
        }
        ItemContent::Stack(inner_group) => {
            // Nested groups ignore their own anchor; the anchor derives
            // from the stack slot the parent assigned to this item.
            let anchor_x = (rect[0] + rect[2]) * 0.5;
            let anchor_y = (rect[1] + rect[3]) * 0.5;

            let mut child_prefix = smallvec_push(path_prefix, item_idx as u8);
            // Guard against design-smell overflow — anything deeper than
            // 4 levels is a bug and the path_prefix slice would exceed
            // ItemPath capacity.
            debug_assert!(
                child_prefix.len() <= 4,
                "decorator nesting exceeded ItemPath depth 4"
            );
            if child_prefix.len() > 4 {
                child_prefix.truncate(4);
            }

            let inner = compute_decorator_group_at(
                inner_group,
                anchor_x,
                anchor_y,
                annotation_id,
                ctx,
                alpha,
                &child_prefix,
            );
            output.merge(inner);

            // Item-level stack action (rare but allowed): emit a hit
            // zone over the whole stack rect.
            if let Some(action) = item.action {
                output.hit_zones.push(HitZone {
                    annotation_id,
                    rect,
                    kind: HitZoneKind::Decorator {
                        group_id,
                        item_path: build_path(path_prefix, &[item_idx as u8]),
                        action,
                    },
                    cursor: CursorIcon::Pointer,
                });
            }
        }
        ItemContent::Spacer(_) => {
            // Intentionally nothing: the spacer was already reserved by
            // the layout pass via its `(w, 0.0)` measurement.
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_badge(
    item_idx: usize,
    badge: &Badge,
    item_action: Option<super::action::DecoratorAction>,
    rect: [f32; 4],
    group_id: u16,
    annotation_id: AnnotationId,
    alpha: f32,
    path_prefix: &[u8],
    output: &mut WidgetOutput,
) {
    // Rect shapes emit via `fills` (cheaper — routed through the grid
    // pipeline); every other shape emits a `BadgeInstance` into `badges`
    // for the SDF `BadgePipeline`.
    let fill = apply_alpha(badge.fill, alpha);
    if matches!(badge.shape, BadgeShape::Rect) {
        output.fills.push(GridLineInstance { rect, color: fill });
    } else {
        output.badges.push(make_badge_instance(
            rect,
            fill,
            badge.shape,
            badge.border,
            alpha,
        ));
    }

    // Recompute per-segment widths so the label and divider X positions
    // line up with the measurement pass. `measure_badge` guarantees the
    // outer width matches the summed segment widths + 2*padding.
    let (_bw, _bh) = measure_badge(badge);
    let mut seg_x = rect[0] + badge.padding;
    let top = rect[1];
    let bottom = rect[3];

    let segments: &[BadgeSegment] = &badge.segments;
    for (seg_idx, segment) in segments.iter().enumerate() {
        let text_w = measure_text(&segment.text, segment.font_size);
        let intrinsic = text_w + badge.padding * 2.0;
        let seg_w = match segment.min_width {
            Some(min) => intrinsic.max(min),
            None => intrinsic,
        };
        let seg_rect = [seg_x, top, seg_x + seg_w, bottom];

        // If the segment overrides the parent's shape or fill, emit an
        // extra `BadgeInstance` covering just the segment's sub-rect.
        // The outer badge instance still covers the body; the segment
        // instance overlays it (e.g. the "black circle around 2" case).
        if segment.shape_override.is_some() || segment.fill_override.is_some() {
            let seg_shape = segment.shape_override.unwrap_or(badge.shape);
            let seg_fill = apply_alpha(segment.fill_override.unwrap_or(badge.fill), alpha);
            output.badges.push(make_badge_instance(
                seg_rect, seg_fill, seg_shape, None, alpha,
            ));
        }

        // Segment label, centered inside the segment rect.
        let label_color = apply_alpha(segment.text_color, alpha);
        output.labels.push(WidgetLabel {
            text: segment.text.clone(),
            screen_x: (seg_rect[0] + seg_rect[2]) * 0.5,
            screen_y: (seg_rect[1] + seg_rect[3]) * 0.5,
            bg_color: [0.0; 4],
            text_color: label_color,
            font_size: segment.font_size,
            anchor: LabelAnchor::Center,
        });

        // Segment-level hit zone (separate from item-level).
        if let Some(action) = segment.action {
            output.hit_zones.push(HitZone {
                annotation_id,
                rect: seg_rect,
                kind: HitZoneKind::Decorator {
                    group_id,
                    item_path: build_path(path_prefix, &[item_idx as u8, seg_idx as u8]),
                    action,
                },
                cursor: CursorIcon::Pointer,
            });
        }

        // Divider between this segment and the next (if configured).
        if let Some(divider) = badge.divider_color {
            if seg_idx + 1 < segments.len() {
                let div_x = seg_rect[2];
                let div_color = apply_alpha(divider, alpha);
                output.fills.push(GridLineInstance {
                    rect: [div_x, top, div_x + 1.0, bottom],
                    color: div_color,
                });
            }
        }

        seg_x += seg_w;
    }

    // Item-level hit zone for a top-level badge action (covers the
    // whole badge rect, separate from per-segment zones).
    if let Some(action) = item_action {
        output.hit_zones.push(HitZone {
            annotation_id,
            rect,
            kind: HitZoneKind::Decorator {
                group_id,
                item_path: build_path(path_prefix, &[item_idx as u8]),
                action,
            },
            cursor: CursorIcon::Pointer,
        });
    }
}

/// Compose `path_prefix ++ suffix` into an `ItemPath`.
fn build_path(path_prefix: &[u8], suffix: &[u8]) -> ItemPath {
    let mut buf = [0u8; 4];
    let mut len = 0usize;
    for &b in path_prefix.iter().chain(suffix.iter()) {
        if len >= 4 {
            break;
        }
        buf[len] = b;
        len += 1;
    }
    ItemPath::new(&buf[..len])
}

/// Build a fresh `SmallVec<u8, 4>`-like buffer by appending one byte.
fn smallvec_push(prefix: &[u8], byte: u8) -> smallvec::SmallVec<[u8; 4]> {
    let mut v: smallvec::SmallVec<[u8; 4]> = smallvec::SmallVec::new();
    v.extend_from_slice(prefix);
    v.push(byte);
    v
}

fn apply_alpha(color: [f32; 4], alpha: f32) -> [f32; 4] {
    [color[0], color[1], color[2], color[3] * alpha]
}

/// Emit the body shape for a [`Button`]. Rect buttons use `fills`
/// (grid pipeline); every other shape routes through the SDF badge
/// pipeline via `BadgeInstance`.
fn emit_button_shape(button: &Button, rect: [f32; 4], alpha: f32, output: &mut WidgetOutput) {
    let fill = apply_alpha(button.fill, alpha);
    if matches!(button.shape, BadgeShape::Rect) {
        output.fills.push(GridLineInstance { rect, color: fill });
    } else {
        output.badges.push(make_badge_instance(
            rect,
            fill,
            button.shape,
            button.border,
            alpha,
        ));
    }
}

/// Build a [`BadgeInstance`] from a rect + shape + optional border,
/// with `alpha` already folded into `fill` (and applied to the border).
fn make_badge_instance(
    rect: [f32; 4],
    fill: [f32; 4],
    shape: BadgeShape,
    border: Option<BadgeBorder>,
    alpha: f32,
) -> BadgeInstance {
    let (border_color, border_thickness) = match border {
        Some(b) => (apply_alpha(b.color, alpha), b.thickness),
        None => ([0.0; 4], 0.0),
    };
    BadgeInstance {
        rect,
        fill,
        border: border_color,
        shape_id: shape.shape_id(),
        shape_param: shape.shape_param(),
        border_thickness,
        _pad: 0.0,
    }
}
