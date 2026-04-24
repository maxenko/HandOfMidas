//! Decorator subsystem tests — slice 5a of the chart-transition plan.
//!
//! ## Coverage budget (plan R18)
//!
//! The legacy `widget::decorator` + `widget::order_bracket::decorators`
//! tests total ≈130 cases and 1,411 LOC. Slice 5a explicitly accepts a
//! 54% cut to 60 tests. Test classes intentionally dropped:
//!
//! - **Badge segment / divider permutations** — the slice 5a
//!   vocabulary has no `BadgeSegment`; callers hand in a finished
//!   `BadgeInstance`. Legacy had ~25 tests in this area.
//! - **Flex-direction / anchor-resolution permutations** — the slice
//!   uses absolute rects, so `Row` vs `Column` + `LeftEdge` /
//!   `RightEdge` / `AtChartRightEdge` / `AtTimestamp` cases (~15
//!   tests) are dropped.
//! - **Nested `Stack` path-prefix breadcrumb encoding** — `ItemPath`
//!   was a 4-byte breadcrumb for nested stacks. With flat groups the
//!   whole concept is gone (~12 tests).
//! - **Per-segment action routing + divider colouring** — the segment
//!   machinery itself is out of scope (~10 tests).
//! - **Pointer-inset geometry** for `AtChartRightEdge` badges (~8 tests).
//!
//! The 60 kept tests cover the every-transition axes the slice calls
//! out (visibility + proximity + hover + drag + sub-z ordering +
//! insertion-order tie-break + button hit-test + empty-group +
//! `HoverState` round-trip). Regression parity for the dropped
//! classes moves to the visual parity harness when a pixel diff
//! surfaces (per plan R18 mitigation note).

use std::borrow::Cow;

use super::*;
use crate::input::Point;
use crate::primitives::{BadgeInstance, LineInstance};
use crate::tools::{ContextMenuAction, ContextMenuItem, ToolEffect};

// ── Helpers ────────────────────────────────────────────────────────────

fn line_in(x0: f32, y0: f32, x1: f32, y1: f32) -> DecoratorItem {
    DecoratorItem::Line(LineInstance {
        x0,
        y0,
        x1,
        y1,
        width_px: 1.0,
        color: [255, 255, 255, 255],
    })
}

fn badge_in(x: f32, y: f32, w: f32, h: f32) -> DecoratorItem {
    DecoratorItem::Badge(BadgeInstance {
        x,
        y,
        w,
        h,
        color: [200, 200, 200, 200],
        text: Cow::Borrowed("b"),
    })
}

fn badge_with_alpha(x: f32, y: f32, w: f32, h: f32, alpha: u8) -> DecoratorItem {
    DecoratorItem::Badge(BadgeInstance {
        x,
        y,
        w,
        h,
        color: [10, 20, 30, alpha],
        text: Cow::Borrowed(""),
    })
}

fn button_in(x: f32, y: f32, w: f32, h: f32, action: ButtonAction) -> DecoratorItem {
    DecoratorItem::Button {
        bounds: Rect::new(x, y, x + w, y + h),
        color: [50, 60, 70, 255],
        label: Cow::Borrowed(""),
        action,
    }
}

fn spacer_in(w: f32, h: f32) -> DecoratorItem {
    DecoratorItem::Spacer { w, h }
}

fn group_always(
    id: u64,
    annotation: u64,
    bounds: Rect,
    items: Vec<DecoratorItem>,
) -> DecoratorGroup {
    DecoratorGroup::always(GroupId(id), annotation, bounds, items)
}

fn group_on_hover(
    id: u64,
    annotation: u64,
    bounds: Rect,
    items: Vec<DecoratorItem>,
) -> DecoratorGroup {
    DecoratorGroup::on_hover(GroupId(id), annotation, bounds, items)
}

fn hover_at(x: f32, y: f32) -> HoverState {
    HoverState {
        cursor_px: Some(Point::new(x, y)),
        ..HoverState::default()
    }
}

// ── Rect basics ────────────────────────────────────────────────────────

#[test]
fn rect_new_normalises_corners() {
    let r = Rect::new(10.0, 20.0, 5.0, 2.0);
    assert_eq!(r.x0, 5.0);
    assert_eq!(r.y0, 2.0);
    assert_eq!(r.x1, 10.0);
    assert_eq!(r.y1, 20.0);
}

#[test]
fn rect_contains_inclusive_edges() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    assert!(r.contains(0.0, 0.0));
    assert!(r.contains(10.0, 10.0));
    assert!(r.contains(5.0, 5.0));
    assert!(!r.contains(10.1, 5.0));
    assert!(!r.contains(5.0, -0.1));
}

#[test]
fn rect_distance_to_inside_point_is_zero() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    assert_eq!(r.distance_to(5.0, 5.0), 0.0);
    assert_eq!(r.distance_to(0.0, 0.0), 0.0);
}

#[test]
fn rect_distance_to_external_point_euclidean() {
    let r = Rect::new(0.0, 0.0, 10.0, 10.0);
    // 3-4-5 triangle to corner (10,10): point (13,14).
    let d = r.distance_to(13.0, 14.0);
    assert!((d - 5.0).abs() < 1e-4, "d={d}");
}

#[test]
fn rect_vertical_distance() {
    let r = Rect::new(0.0, 5.0, 10.0, 15.0);
    assert_eq!(r.vertical_distance(10.0), 0.0);
    assert_eq!(r.vertical_distance(0.0), 5.0);
    assert_eq!(r.vertical_distance(20.0), 5.0);
}

// ── DecoratorItem bounds + drawable ────────────────────────────────────

#[test]
fn decorator_item_line_bounds_span_endpoints() {
    let it = line_in(10.0, 20.0, 30.0, 40.0);
    let b = it.bounds();
    assert_eq!(b.x0, 10.0);
    assert_eq!(b.y0, 20.0);
    assert_eq!(b.x1, 30.0);
    assert_eq!(b.y1, 40.0);
    assert!(it.is_drawable());
}

#[test]
fn decorator_item_badge_bounds_match_xywh() {
    let it = badge_in(5.0, 5.0, 20.0, 10.0);
    let b = it.bounds();
    assert_eq!(b.x0, 5.0);
    assert_eq!(b.y0, 5.0);
    assert_eq!(b.x1, 25.0);
    assert_eq!(b.y1, 15.0);
    assert!(it.is_drawable());
}

#[test]
fn decorator_item_button_bounds_match_rect() {
    let action = ButtonAction::Menu(ContextMenuAction::Edit { id: 1 });
    let it = button_in(0.0, 0.0, 40.0, 20.0, action);
    let b = it.bounds();
    assert_eq!(b.width(), 40.0);
    assert_eq!(b.height(), 20.0);
    assert!(it.is_drawable());
}

#[test]
fn decorator_item_spacer_is_not_drawable() {
    let it = spacer_in(12.0, 4.0);
    assert!(!it.is_drawable());
    let b = it.bounds();
    assert_eq!(b.width(), 0.0);
    assert_eq!(b.height(), 0.0);
}

// ── Visibility: Always ─────────────────────────────────────────────────

#[test]
fn visibility_always_group_always_emits() {
    let g = group_always(
        0,
        1,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        vec![badge_in(0.0, 0.0, 5.0, 5.0)],
    );
    // No hover — still visible.
    assert!(visibility_for(&g, &HoverState::default()));
    // Hover anywhere — still visible.
    assert!(visibility_for(&g, &hover_at(1000.0, 1000.0)));
}

#[test]
fn visibility_always_background_emits_without_cursor() {
    let g = group_always(
        0,
        1,
        Rect::new(0.0, 0.0, 10.0, 10.0),
        vec![badge_in(0.0, 0.0, 5.0, 5.0)],
    );
    let em = emissions_for_group(&g, &HoverState::default());
    assert_eq!(em.len(), 1);
    assert_eq!(em[0].0, SubZ::Background);
}

// ── Visibility: OnHover — cursor-inside parent ─────────────────────────

#[test]
fn visibility_onhover_cursor_inside_parent_emits() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(1, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let h = hover_at(25.0, 25.0);
    assert!(visibility_for(&g, &h));
}

#[test]
fn visibility_onhover_cursor_outside_parent_hides() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(1, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let h = hover_at(200.0, 200.0);
    assert!(!visibility_for(&g, &h));
}

#[test]
fn visibility_onhover_no_cursor_hides() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(1, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    assert!(!visibility_for(&g, &HoverState::default()));
}

#[test]
fn visibility_onhover_emits_zero_items_when_hidden() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(1, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let h = hover_at(500.0, 500.0);
    let em = emissions_for_group(&g, &h);
    assert!(em.is_empty());
}

// ── Visibility: OnHover — annotation hovered ───────────────────────────

#[test]
fn visibility_onhover_annotation_hovered_emits() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(1, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    // Cursor anywhere; owner hover wins.
    let h = HoverState {
        hovered_annotation: Some(7),
        cursor_px: Some(Point::new(9999.0, 9999.0)),
        ..HoverState::default()
    };
    assert!(visibility_for(&g, &h));
}

#[test]
fn visibility_onhover_different_annotation_hovered_hides() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(1, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let h = HoverState {
        hovered_annotation: Some(42),
        ..HoverState::default()
    };
    assert!(!visibility_for(&g, &h));
}

#[test]
fn visibility_onhover_explicit_parent_chain_used() {
    // Group owned by ann=7 but chains off parent=9.
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = DecoratorGroup {
        id: GroupId(2),
        annotation: 7,
        parent_bounds: parent,
        visibility: Visibility::OnHover { parent: Some(9) },
        items: vec![badge_in(10.0, 10.0, 4.0, 4.0)],
    };
    // Hovering the OWNING annotation (7) must not reveal.
    let h = HoverState {
        hovered_annotation: Some(7),
        ..HoverState::default()
    };
    assert!(!visibility_for(&g, &h));
    // Hovering the PARENT (9) reveals.
    let h = HoverState {
        hovered_annotation: Some(9),
        ..HoverState::default()
    };
    assert!(visibility_for(&g, &h));
}

// ── Visibility: OnHover — sticky expansion ─────────────────────────────

#[test]
fn visibility_onhover_sticky_expanded_group_emits() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_on_hover(3, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let h = HoverState {
        expanded_groups: vec![GroupId(3)],
        ..HoverState::default()
    };
    assert!(visibility_for(&g, &h));
    // Even without cursor.
    let h = HoverState {
        expanded_groups: vec![GroupId(3)],
        cursor_px: None,
        ..HoverState::default()
    };
    assert!(visibility_for(&g, &h));
}

#[test]
fn visibility_onhover_sticky_expansion_is_group_scoped() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g_a = group_on_hover(3, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let g_b = group_on_hover(4, 7, parent, vec![badge_in(10.0, 10.0, 4.0, 4.0)]);
    let h = HoverState {
        expanded_groups: vec![GroupId(3)],
        ..HoverState::default()
    };
    assert!(visibility_for(&g_a, &h));
    // Group 4 still hidden — expansion doesn't spill across groups.
    assert!(!visibility_for(&g_b, &h));
}

// ── Proximity promotion: within / outside threshold ────────────────────

#[test]
fn proximity_item_within_32px_promotes() {
    // Badge at (0,0..10,10); cursor at (40, 5) → parent 30 px away.
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(40.0, 5.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted.len(), 1);
    assert_eq!(promoted[0].sub_z, SubZ::Proximity);
}

#[test]
fn proximity_item_outside_32px_stays_background() {
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    // Cursor 100 px away — well beyond threshold.
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(200.0, 200.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Background);
}

#[test]
fn proximity_cursor_none_does_not_promote() {
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    let promoted = promote_by_proximity(&items, None, parent, PROXIMITY_THRESHOLD_PX, false);
    assert_eq!(promoted[0].sub_z, SubZ::Background);
}

#[test]
fn proximity_threshold_is_exactly_32_px() {
    // Cursor exactly 32 px away on the x-axis from the parent bounds.
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    // Item sits at x=[0,10]; cursor at x=42 → 32 px away from x=10.
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(42.0, 5.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Proximity);
    // 32.01 → background.
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(42.01, 5.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Background);
}

#[test]
fn proximity_detects_via_item_bounds_even_far_parent() {
    // Item sits far from the parent bounds; cursor near item only.
    let items = vec![badge_in(500.0, 500.0, 20.0, 20.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0); // far away
                                                  // Cursor 5 px from the item.
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(495.0, 510.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Proximity);
}

#[test]
fn proximity_threshold_parameter_respected() {
    // Force threshold to 5 px; cursor 10 px away must stay background.
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    let promoted = promote_by_proximity(&items, Some(Point::new(20.0, 5.0)), parent, 5.0, false);
    assert_eq!(promoted[0].sub_z, SubZ::Background);
}

// ── Hover sub-z ────────────────────────────────────────────────────────

#[test]
fn hover_over_item_promotes_to_sub_z_2() {
    let items = vec![badge_in(10.0, 10.0, 20.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 200.0, 200.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(15.0, 15.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Hovered);
}

#[test]
fn hover_wins_over_proximity() {
    // Cursor inside the item AND obviously close to parent; hover wins.
    let items = vec![badge_in(10.0, 10.0, 20.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 5.0, 5.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(15.0, 15.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Hovered);
}

#[test]
fn hover_miss_reverts_to_proximity_if_parent_near() {
    // Cursor not over item but close to parent bounds.
    let items = vec![badge_in(100.0, 100.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    // Cursor 10 px outside the parent → proximity (< 32 threshold).
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(60.0, 40.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Proximity);
}

#[test]
fn spacer_never_hovers_or_proximity_promotes_via_own_bounds() {
    // Item is a Spacer — has zero area; only parent-distance counts.
    let items = vec![spacer_in(10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    // Cursor over the nominal spacer (0,0)..(10,10) — but Spacer doesn't
    // report bounds — so only parent bounds count (which contain it).
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(5.0, 5.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    // Parent distance = 0 → proximity band (NOT hovered, spacer isn't drawable).
    assert_eq!(promoted[0].sub_z, SubZ::Proximity);
}

// ── Drag ghost sub-z + alpha ───────────────────────────────────────────

#[test]
fn drag_forces_every_item_to_sub_z_3() {
    let items = vec![
        badge_in(0.0, 0.0, 10.0, 10.0),
        line_in(50.0, 50.0, 100.0, 50.0),
        spacer_in(5.0, 5.0),
    ];
    let parent = Rect::new(0.0, 0.0, 200.0, 200.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(5.0, 5.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        true,
    );
    assert_eq!(promoted.len(), 3);
    for p in &promoted {
        assert_eq!(p.sub_z, SubZ::Dragged);
    }
}

#[test]
fn drag_wins_over_hover() {
    // Cursor directly on the item AND dragged → still Dragged.
    let items = vec![badge_in(10.0, 10.0, 20.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 100.0, 100.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(15.0, 15.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        true,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Dragged);
}

#[test]
fn drag_ghost_alpha_applied_to_badge_color() {
    // Original alpha=200 → 200 * 0.5 = 100.
    let item = badge_with_alpha(0.0, 0.0, 10.0, 10.0, 200);
    let em = DecoratorEmission::from_item(&item, true).unwrap();
    match em {
        DecoratorEmission::Badge(b) => assert_eq!(b.color[3], 100),
        _ => panic!("expected badge"),
    }
}

#[test]
fn drag_ghost_alpha_applied_to_line_color() {
    let item = DecoratorItem::Line(LineInstance {
        x0: 0.0,
        y0: 0.0,
        x1: 10.0,
        y1: 0.0,
        width_px: 1.0,
        color: [10, 20, 30, 240],
    });
    let em = DecoratorEmission::from_item(&item, true).unwrap();
    match em {
        DecoratorEmission::Line(l) => {
            // 240 * 0.5 = 120 (rounded).
            assert_eq!(l.color[3], 120);
            // RGB untouched.
            assert_eq!(l.color[0], 10);
            assert_eq!(l.color[1], 20);
            assert_eq!(l.color[2], 30);
        }
        _ => panic!("expected line"),
    }
}

#[test]
fn drag_ghost_alpha_idempotent_when_ghost_false() {
    let item = badge_with_alpha(0.0, 0.0, 10.0, 10.0, 200);
    let em = DecoratorEmission::from_item(&item, false).unwrap();
    match em {
        DecoratorEmission::Badge(b) => assert_eq!(b.color[3], 200),
        _ => panic!("expected badge"),
    }
}

#[test]
fn drag_ghost_preserves_original_at_sub_z_zero_when_not_dragging() {
    // Dragging another annotation; our group's items keep their sub_z.
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_always(
        1,
        7,
        parent,
        vec![badge_with_alpha(100.0, 100.0, 10.0, 10.0, 255)],
    );
    let h = HoverState {
        dragged_annotation: Some(42), // NOT our annotation
        ..HoverState::default()
    };
    let em = emissions_for_group(&g, &h);
    assert_eq!(em.len(), 1);
    assert_eq!(em[0].0, SubZ::Background);
    match &em[0].2 {
        DecoratorEmission::Badge(b) => assert_eq!(b.color[3], 255, "alpha untouched"),
        _ => panic!("expected badge"),
    }
}

#[test]
fn drag_ghost_of_our_annotation_applies_alpha() {
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let g = group_always(
        1,
        7,
        parent,
        vec![badge_with_alpha(100.0, 100.0, 10.0, 10.0, 200)],
    );
    let h = HoverState {
        dragged_annotation: Some(7),
        ..HoverState::default()
    };
    let em = emissions_for_group(&g, &h);
    assert_eq!(em[0].0, SubZ::Dragged);
    match &em[0].2 {
        DecoratorEmission::Badge(b) => assert_eq!(b.color[3], 100),
        _ => panic!("expected badge"),
    }
}

#[test]
fn drag_ghost_alpha_clamps_at_zero_for_fully_transparent_input() {
    let item = badge_with_alpha(0.0, 0.0, 10.0, 10.0, 0);
    let em = DecoratorEmission::from_item(&item, true).unwrap();
    match em {
        DecoratorEmission::Badge(b) => assert_eq!(b.color[3], 0),
        _ => panic!("expected badge"),
    }
}

// ── Sub-z ordering ─────────────────────────────────────────────────────

#[test]
fn sub_z_ordinal_matches_spec() {
    assert_eq!(SubZ::Background.ordinal(), 0);
    assert_eq!(SubZ::Proximity.ordinal(), 1);
    assert_eq!(SubZ::Hovered.ordinal(), 2);
    assert_eq!(SubZ::Dragged.ordinal(), 3);
}

#[test]
fn sub_z_ordering_is_total() {
    assert!(SubZ::Background < SubZ::Proximity);
    assert!(SubZ::Proximity < SubZ::Hovered);
    assert!(SubZ::Hovered < SubZ::Dragged);
}

#[test]
fn emissions_preserve_insertion_order_within_same_sub_z() {
    // Two background items: expect order 0, 1.
    let parent = Rect::new(0.0, 0.0, 5.0, 5.0);
    let items = vec![
        badge_in(1000.0, 1000.0, 4.0, 4.0), // far
        badge_in(2000.0, 2000.0, 4.0, 4.0), // farther
    ];
    let g = group_always(1, 7, parent, items);
    // Cursor far from everything so both are background.
    let em = emissions_for_group(
        &g,
        &HoverState {
            cursor_px: Some(Point::new(0.0, 0.0)),
            ..HoverState::default()
        },
    );
    assert_eq!(em.len(), 2);
    assert!(em[0].1 < em[1].1, "insertion-index preserved");
}

// ── Emissions integration ──────────────────────────────────────────────

#[test]
fn emissions_empty_when_no_items() {
    let g = group_always(1, 7, Rect::new(0.0, 0.0, 10.0, 10.0), vec![]);
    let em = emissions_for_group(&g, &HoverState::default());
    assert!(em.is_empty());
}

#[test]
fn emissions_skip_spacers() {
    let items = vec![
        badge_in(0.0, 0.0, 10.0, 10.0),
        spacer_in(10.0, 10.0),
        badge_in(30.0, 0.0, 10.0, 10.0),
    ];
    let g = group_always(1, 7, Rect::new(0.0, 0.0, 100.0, 100.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    // 3 items, one spacer, 2 emissions.
    assert_eq!(em.len(), 2);
}

#[test]
fn emissions_attach_correct_sub_z_per_item() {
    // Cursor directly over item 0; item 1 is far enough to be background.
    let items = vec![
        badge_in(10.0, 10.0, 20.0, 10.0),
        badge_in(500.0, 500.0, 10.0, 10.0),
    ];
    let parent = Rect::new(0.0, 0.0, 30.0, 30.0);
    let g = group_always(1, 7, parent, items);
    let em = emissions_for_group(
        &g,
        &HoverState {
            cursor_px: Some(Point::new(15.0, 15.0)),
            ..HoverState::default()
        },
    );
    assert_eq!(em.len(), 2);
    // Item 0: cursor inside → hovered.
    assert_eq!(em[0].0, SubZ::Hovered);
    // Item 1: far → background (parent bounds close to cursor,
    // but item's own bounds far; parent distance <= 32 so Proximity
    // — sanity check per the spec).
    // Cursor = (15,15), parent = (0..30, 0..30) → inside parent → 0.
    // Therefore item 1 lands in Proximity (parent distance < 32).
    assert_eq!(em[1].0, SubZ::Proximity);
}

#[test]
fn emissions_line_propagates_color() {
    let line = LineInstance {
        x0: 0.0,
        y0: 5.0,
        x1: 100.0,
        y1: 5.0,
        width_px: 2.0,
        color: [7, 8, 9, 210],
    };
    let g = group_always(
        1,
        7,
        Rect::new(0.0, 0.0, 100.0, 10.0),
        vec![DecoratorItem::Line(line)],
    );
    let em = emissions_for_group(&g, &HoverState::default());
    match &em[0].2 {
        DecoratorEmission::Line(l) => {
            assert_eq!(l.color, [7, 8, 9, 210]);
            assert_eq!(l.width_px, 2.0);
        }
        _ => panic!("expected line"),
    }
}

// ── HoverState round-trip ──────────────────────────────────────────────

#[test]
fn hover_state_default_has_no_cursor_no_hover_no_drag() {
    let h = HoverState::default();
    assert!(h.hovered_annotation.is_none());
    assert!(h.dragged_annotation.is_none());
    assert!(h.cursor_px.is_none());
    assert!(h.expanded_groups.is_empty());
}

#[test]
fn hover_state_is_annotation_hovered() {
    let h = HoverState::default();
    assert!(!h.is_annotation_hovered(7));
    let h = HoverState {
        hovered_annotation: Some(7),
        ..HoverState::default()
    };
    assert!(h.is_annotation_hovered(7));
    assert!(!h.is_annotation_hovered(42));
}

#[test]
fn hover_state_is_group_expanded() {
    let h = HoverState::default();
    assert!(!h.is_group_expanded(GroupId(3)));
    let h = HoverState {
        expanded_groups: vec![GroupId(3)],
        ..HoverState::default()
    };
    assert!(h.is_group_expanded(GroupId(3)));
    assert!(!h.is_group_expanded(GroupId(4)));
}

#[test]
fn hover_state_is_annotation_dragged() {
    let h = HoverState::default();
    assert!(!h.is_annotation_dragged(7));
    let h = HoverState {
        dragged_annotation: Some(7),
        ..HoverState::default()
    };
    assert!(h.is_annotation_dragged(7));
}

#[test]
fn hover_state_cloneable_and_equatable() {
    let h1 = HoverState {
        hovered_annotation: Some(1),
        dragged_annotation: Some(2),
        expanded_groups: vec![GroupId(5)],
        cursor_px: Some(Point::new(10.0, 20.0)),
    };
    let h2 = h1.clone();
    assert_eq!(h1, h2);
}

// ── Layer integration (DecoratorLayer paint) ───────────────────────────

#[test]
fn decorator_layer_paints_every_visible_group() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    let groups = vec![
        group_always(
            1,
            7,
            Rect::new(0.0, 0.0, 100.0, 100.0),
            vec![badge_in(10.0, 10.0, 20.0, 20.0)],
        ),
        group_always(
            2,
            8,
            Rect::new(100.0, 0.0, 200.0, 100.0),
            vec![line_in(100.0, 50.0, 200.0, 50.0)],
        ),
    ];
    let layer = DecoratorLayer::new(groups, HoverState::default());
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.badges.len(), 1);
    assert_eq!(out.lines.len(), 1);
}

#[test]
fn decorator_layer_skips_hidden_groups() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    let groups = vec![group_on_hover(
        1,
        7,
        Rect::new(0.0, 0.0, 100.0, 100.0),
        vec![badge_in(10.0, 10.0, 20.0, 20.0)],
    )];
    // No cursor, no hover, no expansion — group is hidden.
    let layer = DecoratorLayer::new(groups, HoverState::default());
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert!(out.badges.is_empty());
}

#[test]
fn decorator_layer_paints_higher_sub_z_after_lower() {
    // Two groups with same insertion-order; one is background, the
    // other hovered. The hovered one must emit after the background.
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    // Group A (ann 1) — background (cursor far from items).
    let g_a = group_always(
        1,
        1,
        Rect::new(1000.0, 1000.0, 1100.0, 1100.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 1000.0,
            y: 1000.0,
            w: 10.0,
            h: 10.0,
            color: [0xAA, 0, 0, 255],
            text: Cow::Borrowed("A"),
        })],
    );
    // Group B (ann 2) — cursor directly over → hovered.
    let g_b = group_always(
        2,
        2,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 10.0,
            y: 10.0,
            w: 10.0,
            h: 10.0,
            color: [0xBB, 0, 0, 255],
            text: Cow::Borrowed("B"),
        })],
    );
    let hover = HoverState {
        cursor_px: Some(Point::new(15.0, 15.0)),
        ..HoverState::default()
    };
    let layer = DecoratorLayer::new(vec![g_a, g_b], hover);
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.badges.len(), 2);
    // Background item paints first; hovered second → hovered on top (higher index in Vec).
    assert_eq!(out.badges[0].color[0], 0xAA, "background first");
    assert_eq!(out.badges[1].color[0], 0xBB, "hovered after");
}

#[test]
fn decorator_layer_preserves_group_insertion_order_within_same_sub_z() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    // Both groups are background — order should be group-insertion order.
    let g_a = group_always(
        1,
        1,
        Rect::new(1000.0, 1000.0, 1100.0, 1100.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 1000.0,
            y: 1000.0,
            w: 10.0,
            h: 10.0,
            color: [0xAA, 0, 0, 255],
            text: Cow::Borrowed("A"),
        })],
    );
    let g_b = group_always(
        2,
        2,
        Rect::new(2000.0, 2000.0, 2100.0, 2100.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 2000.0,
            y: 2000.0,
            w: 10.0,
            h: 10.0,
            color: [0xBB, 0, 0, 255],
            text: Cow::Borrowed("B"),
        })],
    );
    let layer = DecoratorLayer::new(vec![g_b, g_a], HoverState::default());
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    // Insertion was [g_b, g_a]; both background; order follows.
    assert_eq!(out.badges[0].color[0], 0xBB);
    assert_eq!(out.badges[1].color[0], 0xAA);
}

// ── Button hit-test ────────────────────────────────────────────────────

#[test]
fn button_hit_test_returns_some_inside_rect() {
    use crate::layers::DecoratorLayer;
    let action = ButtonAction::Menu(ContextMenuAction::Edit { id: 42 });
    let btn = button_in(10.0, 10.0, 20.0, 20.0, action);
    let g = group_always(1, 42, Rect::new(0.0, 0.0, 50.0, 50.0), vec![btn]);
    let layer = DecoratorLayer::new(vec![g], HoverState::default());
    // Hit center of the button.
    let hit = layer.button_at(Point::new(20.0, 20.0));
    assert!(hit.is_some());
}

#[test]
fn button_hit_test_returns_none_outside_rect() {
    use crate::layers::DecoratorLayer;
    let action = ButtonAction::Menu(ContextMenuAction::Edit { id: 42 });
    let btn = button_in(10.0, 10.0, 20.0, 20.0, action);
    let g = group_always(1, 42, Rect::new(0.0, 0.0, 50.0, 50.0), vec![btn]);
    let layer = DecoratorLayer::new(vec![g], HoverState::default());
    let hit = layer.button_at(Point::new(100.0, 100.0));
    assert!(hit.is_none());
}

#[test]
fn button_click_on_menu_action_emits_menu_effect() {
    use crate::input::{InputEvent, MouseButton};
    use crate::layer::{InteractiveLayer, ToolContext};
    use crate::layers::DecoratorLayer;
    let action = ButtonAction::Menu(ContextMenuAction::Delete { id: 99 });
    let btn = button_in(10.0, 10.0, 20.0, 20.0, action);
    let g = group_always(1, 99, Rect::new(0.0, 0.0, 50.0, 50.0), vec![btn]);
    let mut layer = DecoratorLayer::new(vec![g], HoverState::default());

    let pr = midas_axis::PriceRange::new(90.0, 110.0).unwrap();
    let mut effects: Vec<ToolEffect> = Vec::new();
    let mut last_err: Option<crate::error::SceneError> = None;
    let mut ctx = ToolContext {
        price_range: &pr,
        last_error: &mut last_err,
        effects: &mut effects,
    };
    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(15.0, 15.0),
        modifiers: crate::input::Modifiers::default(),
    };
    let status = layer.update(ev, &mut ctx);
    assert_eq!(status, crate::input::EventStatus::Captured);
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        ToolEffect::DeleteLevel { id } => assert_eq!(*id, 99),
        other => panic!("expected DeleteLevel, got {other:?}"),
    }
}

#[test]
fn button_click_on_open_context_menu_emits_open_menu_effect() {
    use crate::input::{InputEvent, MouseButton};
    use crate::layer::{InteractiveLayer, ToolContext};
    use crate::layers::DecoratorLayer;
    let items = vec![ContextMenuItem {
        label: "Edit".into(),
        action: ContextMenuAction::Edit { id: 1 },
    }];
    let btn = button_in(
        10.0,
        10.0,
        20.0,
        20.0,
        ButtonAction::OpenContextMenu {
            items: items.clone(),
        },
    );
    let g = group_always(1, 1, Rect::new(0.0, 0.0, 50.0, 50.0), vec![btn]);
    let mut layer = DecoratorLayer::new(vec![g], HoverState::default());

    let pr = midas_axis::PriceRange::new(90.0, 110.0).unwrap();
    let mut effects: Vec<ToolEffect> = Vec::new();
    let mut last_err: Option<crate::error::SceneError> = None;
    let mut ctx = ToolContext {
        price_range: &pr,
        last_error: &mut last_err,
        effects: &mut effects,
    };
    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(15.0, 15.0),
        modifiers: crate::input::Modifiers::default(),
    };
    layer.update(ev, &mut ctx);
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        ToolEffect::OpenContextMenu { items: got, .. } => {
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].label, "Edit");
        }
        other => panic!("expected OpenContextMenu, got {other:?}"),
    }
}

#[test]
fn button_click_on_effect_variant_forwards_effect_verbatim() {
    use crate::input::{InputEvent, MouseButton};
    use crate::layer::{InteractiveLayer, ToolContext};
    use crate::layers::DecoratorLayer;
    let effect_payload = ToolEffect::UpdateLevel {
        id: 55,
        price: 123.45,
    };
    let btn = button_in(
        0.0,
        0.0,
        10.0,
        10.0,
        ButtonAction::Effect(effect_payload.clone()),
    );
    let g = group_always(1, 55, Rect::new(0.0, 0.0, 50.0, 50.0), vec![btn]);
    let mut layer = DecoratorLayer::new(vec![g], HoverState::default());

    let pr = midas_axis::PriceRange::new(90.0, 110.0).unwrap();
    let mut effects: Vec<ToolEffect> = Vec::new();
    let mut last_err: Option<crate::error::SceneError> = None;
    let mut ctx = ToolContext {
        price_range: &pr,
        last_error: &mut last_err,
        effects: &mut effects,
    };
    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(5.0, 5.0),
        modifiers: crate::input::Modifiers::default(),
    };
    layer.update(ev, &mut ctx);
    assert_eq!(effects, vec![effect_payload]);
}

#[test]
fn button_click_on_hidden_group_is_not_routed() {
    use crate::input::{InputEvent, MouseButton};
    use crate::layer::{InteractiveLayer, ToolContext};
    use crate::layers::DecoratorLayer;
    let btn = button_in(
        10.0,
        10.0,
        20.0,
        20.0,
        ButtonAction::Menu(ContextMenuAction::Edit { id: 1 }),
    );
    // OnHover, no cursor, no hover → hidden.
    let g = group_on_hover(1, 99, Rect::new(0.0, 0.0, 50.0, 50.0), vec![btn]);
    let mut layer = DecoratorLayer::new(vec![g], HoverState::default());

    let pr = midas_axis::PriceRange::new(90.0, 110.0).unwrap();
    let mut effects: Vec<ToolEffect> = Vec::new();
    let mut last_err: Option<crate::error::SceneError> = None;
    let mut ctx = ToolContext {
        price_range: &pr,
        last_error: &mut last_err,
        effects: &mut effects,
    };
    let ev = InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(15.0, 15.0),
        modifiers: crate::input::Modifiers::default(),
    };
    let status = layer.update(ev, &mut ctx);
    assert_eq!(status, crate::input::EventStatus::Ignored);
    assert!(effects.is_empty());
}

#[test]
fn mouse_up_and_move_events_are_ignored_by_decorator_layer() {
    use crate::input::{InputEvent, MouseButton};
    use crate::layer::{InteractiveLayer, ToolContext};
    use crate::layers::DecoratorLayer;
    let mut layer = DecoratorLayer::new(vec![], HoverState::default());

    let pr = midas_axis::PriceRange::new(90.0, 110.0).unwrap();
    let mut effects: Vec<ToolEffect> = Vec::new();
    let mut last_err: Option<crate::error::SceneError> = None;
    let mut ctx = ToolContext {
        price_range: &pr,
        last_error: &mut last_err,
        effects: &mut effects,
    };
    let status = layer.update(
        InputEvent::MouseMove {
            pt: Point::new(0.0, 0.0),
        },
        &mut ctx,
    );
    assert_eq!(status, crate::input::EventStatus::Ignored);
    let status = layer.update(
        InputEvent::MouseUp {
            button: MouseButton::Left,
            pt: Point::new(0.0, 0.0),
        },
        &mut ctx,
    );
    assert_eq!(status, crate::input::EventStatus::Ignored);
}

// ── as_interactive ─────────────────────────────────────────────────────

#[test]
fn decorator_layer_as_interactive_returns_self() {
    use crate::layer::SceneLayer;
    use crate::layers::DecoratorLayer;
    let mut layer = DecoratorLayer::new(vec![], HoverState::default());
    assert!(layer.as_interactive().is_some());
}

// ── Empty / degenerate cases ───────────────────────────────────────────

#[test]
fn emissions_empty_group_produces_empty_vec() {
    let g = group_always(0, 0, Rect::new(0.0, 0.0, 10.0, 10.0), vec![]);
    let em = emissions_for_group(&g, &HoverState::default());
    assert!(em.is_empty());
}

#[test]
fn emissions_on_hover_group_with_no_cursor_empty() {
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let g = group_on_hover(0, 0, Rect::new(0.0, 0.0, 10.0, 10.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    assert!(em.is_empty());
}

#[test]
fn decorator_layer_with_zero_groups_paints_nothing() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    let layer = DecoratorLayer::new(vec![], HoverState::default());
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert!(out.is_empty());
}

// ── Bracket-shaped integration (3 sibling groups) ──────────────────────

#[test]
fn decorator_layer_three_sibling_groups_interleave_by_sub_z() {
    // Simulates the heaviest consumer shape (slice 5b): one bracket =
    // three decorator groups (entry + TP + SL). Only one of them is
    // dragged; the dragged one must land on top.
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    // ann 100 = "entry leg" (not dragged)
    let g_entry = group_always(
        1,
        100,
        Rect::new(0.0, 50.0, 200.0, 60.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 190.0,
            y: 52.0,
            w: 10.0,
            h: 6.0,
            color: [0x11, 0, 0, 255],
            text: Cow::Borrowed("E"),
        })],
    );
    // ann 101 = "TP leg" (being dragged)
    let g_tp = group_always(
        2,
        101,
        Rect::new(0.0, 20.0, 200.0, 30.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 190.0,
            y: 22.0,
            w: 10.0,
            h: 6.0,
            color: [0x22, 0, 0, 255],
            text: Cow::Borrowed("TP"),
        })],
    );
    // ann 102 = "SL leg" (not dragged)
    let g_sl = group_always(
        3,
        102,
        Rect::new(0.0, 100.0, 200.0, 110.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 190.0,
            y: 102.0,
            w: 10.0,
            h: 6.0,
            color: [0x33, 0, 0, 255],
            text: Cow::Borrowed("SL"),
        })],
    );
    let hover = HoverState {
        dragged_annotation: Some(101),
        cursor_px: Some(Point::new(-9999.0, -9999.0)), // far from everything
        ..HoverState::default()
    };
    let layer = DecoratorLayer::new(vec![g_entry, g_tp, g_sl], hover);
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.badges.len(), 3);
    // Order must be entry, SL (both background), then TP (dragged, on top).
    // Background insertion order is entry(1), SL(3). Dragged comes last.
    assert_eq!(out.badges[0].color[0], 0x11, "entry first (bg)");
    assert_eq!(out.badges[1].color[0], 0x33, "sl second (bg)");
    assert_eq!(out.badges[2].color[0], 0x22, "tp last (dragged)");
    // Dragged must have blended alpha.
    assert_eq!(out.badges[2].color[3], (255.0f32 * 0.5).round() as u8);
}

#[test]
fn decorator_layer_three_sibling_groups_all_background_preserve_insertion() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    let g1 = group_always(
        1,
        100,
        Rect::new(0.0, 50.0, 200.0, 60.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 190.0,
            y: 52.0,
            w: 10.0,
            h: 6.0,
            color: [0xAA, 0, 0, 255],
            text: Cow::Borrowed(""),
        })],
    );
    let g2 = group_always(
        2,
        101,
        Rect::new(0.0, 20.0, 200.0, 30.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 190.0,
            y: 22.0,
            w: 10.0,
            h: 6.0,
            color: [0xBB, 0, 0, 255],
            text: Cow::Borrowed(""),
        })],
    );
    let g3 = group_always(
        3,
        102,
        Rect::new(0.0, 100.0, 200.0, 110.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 190.0,
            y: 102.0,
            w: 10.0,
            h: 6.0,
            color: [0xCC, 0, 0, 255],
            text: Cow::Borrowed(""),
        })],
    );
    // Cursor far so all three are background.
    let hover = HoverState {
        cursor_px: Some(Point::new(-9999.0, -9999.0)),
        ..HoverState::default()
    };
    let layer = DecoratorLayer::new(vec![g1, g2, g3], hover);
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.badges.len(), 3);
    assert_eq!(out.badges[0].color[0], 0xAA);
    assert_eq!(out.badges[1].color[0], 0xBB);
    assert_eq!(out.badges[2].color[0], 0xCC);
}

// ── Sub-z interleave across two items in the same group ───────────────

#[test]
fn decorator_layer_items_within_group_sort_by_sub_z() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    // Group with two items; item 0 far away, item 1 under cursor.
    let items = vec![
        // item 0 — will land in Background (beyond threshold)
        DecoratorItem::Badge(BadgeInstance {
            x: 500.0,
            y: 500.0,
            w: 10.0,
            h: 10.0,
            color: [0x11, 0, 0, 255],
            text: Cow::Borrowed("far"),
        }),
        // item 1 — cursor is on it → Hovered
        DecoratorItem::Badge(BadgeInstance {
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 20.0,
            color: [0x22, 0, 0, 255],
            text: Cow::Borrowed("near"),
        }),
    ];
    // Parent far away so item 0 is not near parent either.
    let g = group_always(1, 7, Rect::new(400.0, 400.0, 520.0, 520.0), items);
    let hover = HoverState {
        cursor_px: Some(Point::new(15.0, 15.0)),
        ..HoverState::default()
    };
    let layer = DecoratorLayer::new(vec![g], hover);
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.badges.len(), 2);
    // Background item 0 (0x11) first, hovered item 1 (0x22) last.
    assert_eq!(out.badges[0].color[0], 0x11);
    assert_eq!(out.badges[1].color[0], 0x22);
}

// ── Button decorations still emit as badges ────────────────────────────

#[test]
fn button_emission_has_badge_rect_and_color() {
    let action = ButtonAction::Menu(ContextMenuAction::ToggleLock { id: 3 });
    let items = vec![DecoratorItem::Button {
        bounds: Rect::new(5.0, 5.0, 25.0, 15.0),
        color: [9, 10, 11, 180],
        label: Cow::Borrowed("lock"),
        action,
    }];
    let g = group_always(1, 3, Rect::new(0.0, 0.0, 30.0, 30.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    assert_eq!(em.len(), 1);
    match &em[0].2 {
        DecoratorEmission::Badge(b) => {
            assert_eq!(b.x, 5.0);
            assert_eq!(b.y, 5.0);
            assert_eq!(b.w, 20.0);
            assert_eq!(b.h, 10.0);
            assert_eq!(b.color, [9, 10, 11, 180]);
            assert_eq!(b.text, Cow::Borrowed("lock"));
        }
        _ => panic!("expected badge"),
    }
}

#[test]
fn button_emission_ghost_alpha_applied_when_dragged() {
    let action = ButtonAction::Menu(ContextMenuAction::ToggleLock { id: 3 });
    let items = vec![DecoratorItem::Button {
        bounds: Rect::new(5.0, 5.0, 25.0, 15.0),
        color: [9, 10, 11, 200],
        label: Cow::Borrowed("lock"),
        action,
    }];
    let g = group_always(1, 3, Rect::new(0.0, 0.0, 30.0, 30.0), items);
    let h = HoverState {
        dragged_annotation: Some(3),
        ..HoverState::default()
    };
    let em = emissions_for_group(&g, &h);
    match &em[0].2 {
        DecoratorEmission::Badge(b) => assert_eq!(b.color[3], 100), // 200 * 0.5
        _ => panic!("expected badge"),
    }
}

// ── Extra proximity / parent-bounds edge cases ─────────────────────────

#[test]
fn proximity_parent_bounds_inside_cursor_promotes_distant_item() {
    // Parent contains cursor; far-away item promoted because parent is
    // within threshold (0 distance).
    let items = vec![badge_in(2000.0, 2000.0, 10.0, 10.0)];
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(25.0, 25.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Proximity);
}

#[test]
fn empty_items_returns_empty_promoted() {
    let parent = Rect::new(0.0, 0.0, 10.0, 10.0);
    let promoted = promote_by_proximity(
        &[],
        Some(Point::new(5.0, 5.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        false,
    );
    assert!(promoted.is_empty());
}

#[test]
fn visibility_for_always_group_with_no_parent_bounds_still_visible() {
    // Degenerate — some callers may pass a zero-area parent_bounds.
    let g = group_always(
        1,
        7,
        Rect::new(0.0, 0.0, 0.0, 0.0),
        vec![badge_in(0.0, 0.0, 10.0, 10.0)],
    );
    assert!(visibility_for(&g, &HoverState::default()));
}

// ── Sub-z ordering boundary: Proximity vs Background ──────────────────

#[test]
fn sub_z_bands_emit_in_ascending_order_across_groups() {
    use crate::layers::DecoratorLayer;
    use chrono::TimeZone;
    use midas_axis::{ContinuousAxis, DefaultFormatter, LinearPriceAxis, PriceRange, Viewport};

    let axis = ContinuousAxis::new(
        chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        chrono::Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        1000.0,
    )
    .unwrap();
    let pr = PriceRange::new(90.0, 110.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    let paxis = LinearPriceAxis::new(pr, vp.height_px);
    let pal = crate::ThemePalette::dark_default();
    let fmt = DefaultFormatter::new();

    // Four groups, one per band.
    let g_bg = group_always(
        1,
        1,
        Rect::new(1000.0, 1000.0, 1010.0, 1010.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 1000.0,
            y: 1000.0,
            w: 10.0,
            h: 10.0,
            color: [0, 0, 0, 255],
            text: Cow::Borrowed("bg"),
        })],
    );
    // Proximity: parent near cursor but item far.
    let g_prox = group_always(
        2,
        2,
        Rect::new(40.0, 40.0, 60.0, 60.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 1000.0,
            y: 1000.0,
            w: 10.0,
            h: 10.0,
            color: [1, 0, 0, 255],
            text: Cow::Borrowed("prox"),
        })],
    );
    // Hovered: cursor directly on item.
    let g_hov = group_always(
        3,
        3,
        Rect::new(0.0, 0.0, 5.0, 5.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 10.0,
            y: 10.0,
            w: 20.0,
            h: 20.0,
            color: [2, 0, 0, 255],
            text: Cow::Borrowed("hov"),
        })],
    );
    // Dragged.
    let g_drag = group_always(
        4,
        4,
        Rect::new(200.0, 200.0, 210.0, 210.0),
        vec![DecoratorItem::Badge(BadgeInstance {
            x: 200.0,
            y: 200.0,
            w: 10.0,
            h: 10.0,
            color: [3, 0, 0, 255],
            text: Cow::Borrowed("drag"),
        })],
    );
    let hover = HoverState {
        cursor_px: Some(Point::new(15.0, 15.0)),
        dragged_annotation: Some(4),
        ..HoverState::default()
    };
    let layer = DecoratorLayer::new(vec![g_bg, g_prox, g_hov, g_drag], hover);
    let mut out = crate::primitives::ScenePrimitives::default();
    let mut ctx = crate::paint::PaintContext {
        axis: &axis,
        viewport: vp,
        price_range: pr,
        palette: &pal,
        price_axis: &paxis,
        formatter: &fmt,
        out: &mut out,
    };
    crate::layer::SceneLayer::paint(&layer, &mut ctx);
    assert_eq!(out.badges.len(), 4);
    // bg → prox → hov → drag by r[0] == 0,1,2,3.
    assert_eq!(out.badges[0].color[0], 0);
    assert_eq!(out.badges[1].color[0], 1);
    assert_eq!(out.badges[2].color[0], 2);
    assert_eq!(out.badges[3].color[0], 3);
}

// ── Apply-alpha unit tests ─────────────────────────────────────────────

#[test]
fn apply_drag_ghost_alpha_half_value() {
    let blended = crate::decorator::layout::apply_drag_ghost_alpha([10, 20, 30, 200]);
    assert_eq!(blended, [10, 20, 30, 100]);
}

#[test]
fn apply_drag_ghost_alpha_preserves_rgb() {
    let blended = crate::decorator::layout::apply_drag_ghost_alpha([1, 2, 3, 100]);
    assert_eq!(blended[0], 1);
    assert_eq!(blended[1], 2);
    assert_eq!(blended[2], 3);
}

#[test]
fn apply_drag_ghost_alpha_max_input_saturates() {
    let blended = crate::decorator::layout::apply_drag_ghost_alpha([0, 0, 0, 255]);
    // 255 * 0.5 = 127.5 → rounds to 128.
    assert_eq!(blended[3], 128);
}

// ── GroupId basics ─────────────────────────────────────────────────────

#[test]
fn group_id_is_hashable_and_copy() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(GroupId(1));
    set.insert(GroupId(2));
    set.insert(GroupId(1)); // dup
    assert_eq!(set.len(), 2);
    let g = GroupId(42);
    let g2 = g; // Copy
    assert_eq!(g, g2);
}

// ── Combined drag-then-cursor test ─────────────────────────────────────

#[test]
fn drag_suppresses_proximity_and_hover() {
    // Even if cursor is directly over the item, dragging wins.
    let items = vec![badge_in(10.0, 10.0, 20.0, 20.0)];
    let parent = Rect::new(0.0, 0.0, 50.0, 50.0);
    let promoted = promote_by_proximity(
        &items,
        Some(Point::new(15.0, 15.0)),
        parent,
        PROXIMITY_THRESHOLD_PX,
        /* dragged = */ true,
    );
    assert_eq!(promoted[0].sub_z, SubZ::Dragged);
}

#[test]
fn visibility_for_on_hover_parent_bounds_work_even_if_cursor_none() {
    // If annotation is hovered via input state, the cursor-in-bounds
    // fallback isn't needed.
    let g = group_on_hover(
        1,
        7,
        Rect::new(0.0, 0.0, 50.0, 50.0),
        vec![badge_in(0.0, 0.0, 5.0, 5.0)],
    );
    let h = HoverState {
        cursor_px: None,
        hovered_annotation: Some(7),
        ..HoverState::default()
    };
    assert!(visibility_for(&g, &h));
}

// ── Exhaustive item-variant emission coverage ──────────────────────────

#[test]
fn line_only_group_emits_one_line() {
    let items = vec![line_in(0.0, 5.0, 100.0, 5.0)];
    let g = group_always(1, 1, Rect::new(0.0, 0.0, 100.0, 10.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    assert_eq!(em.len(), 1);
    matches!(&em[0].2, DecoratorEmission::Line(_));
}

#[test]
fn badge_only_group_emits_one_badge() {
    let items = vec![badge_in(0.0, 0.0, 10.0, 10.0)];
    let g = group_always(1, 1, Rect::new(0.0, 0.0, 10.0, 10.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    assert_eq!(em.len(), 1);
    matches!(&em[0].2, DecoratorEmission::Badge(_));
}

#[test]
fn button_only_group_emits_one_badge_shape() {
    let items = vec![button_in(
        0.0,
        0.0,
        10.0,
        10.0,
        ButtonAction::Menu(ContextMenuAction::Edit { id: 1 }),
    )];
    let g = group_always(1, 1, Rect::new(0.0, 0.0, 10.0, 10.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    assert_eq!(em.len(), 1);
    matches!(&em[0].2, DecoratorEmission::Badge(_));
}

#[test]
fn spacer_only_group_emits_nothing() {
    let items = vec![spacer_in(10.0, 10.0)];
    let g = group_always(1, 1, Rect::new(0.0, 0.0, 10.0, 10.0), items);
    let em = emissions_for_group(&g, &HoverState::default());
    assert!(em.is_empty());
}

// ── End of slice-5a test suite ─────────────────────────────────────────
