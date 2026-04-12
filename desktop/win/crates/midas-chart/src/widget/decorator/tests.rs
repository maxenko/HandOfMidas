//! Decorator tests: type-level serde round-trips, GPU shape-id invariants,
//! compute/layout engine, and hover-gated visibility.

use super::*;
use crate::widget::level::LineStyle;
use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
use smallvec::smallvec;

fn sample_badge() -> Badge {
    Badge {
        shape: BadgeShape::PointLeft { point_width: 6.0 },
        fill: [0.2, 0.78, 0.35, 1.0],
        border: Some(BadgeBorder {
            color: [1.0, 1.0, 1.0, 0.4],
            thickness: 1.0,
        }),
        height: 18.0,
        padding: 4.0,
        segments: smallvec![
            BadgeSegment {
                text: "P".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(12.0),
                fill_override: None,
                shape_override: None,
                action: Some(DecoratorAction::CycleEntryType),
            },
            BadgeSegment {
                text: "100".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(24.0),
                fill_override: None,
                shape_override: None,
                action: Some(DecoratorAction::EditQuantity),
            },
        ],
        divider_color: Some([0.0, 0.0, 0.0, 0.4]),
    }
}

fn sample_button() -> Button {
    Button {
        shape: BadgeShape::Rounded { radius: 2.0 },
        fill: [0.9, 0.25, 0.25, 1.0],
        hover_fill: Some([1.0, 0.35, 0.35, 1.0]),
        glyph: 'X',
        glyph_color: [1.0; 4],
        glyph_size: 11.0,
        size: [14.0, 14.0],
        border: None,
    }
}

fn sample_group() -> DecoratorGroup {
    DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 2.0,
        items: smallvec![
            DecoratorItem {
                visibility: Visibility::OnGroupHover,
                action: Some(DecoratorAction::CloseAnnotation),
                content: ItemContent::Button(sample_button()),
            },
            DecoratorItem {
                visibility: Visibility::Always,
                action: None,
                content: ItemContent::Badge(Box::new(sample_badge())),
            },
            DecoratorItem {
                visibility: Visibility::Always,
                action: None,
                content: ItemContent::Spacer(4.0),
            },
        ],
    }
}

fn sample_price_line() -> PriceLine {
    PriceLine {
        price: 185.50,
        extent: LineExtent::FullWidth,
        stroke: LineStroke {
            color: [0.2, 0.78, 0.35, 1.0],
            width: 1.5,
            style: LineStyle::dashed(),
        },
    }
}

#[test]
fn price_line_serde_round_trip() {
    let pl = sample_price_line();
    let json = serde_json::to_string(&pl).expect("serialise");
    let decoded: PriceLine = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(pl, decoded);
}

#[test]
fn line_style_pattern_serialises() {
    let style = LineStyle::Pattern(smallvec![6.0, 3.0, 1.0, 3.0]);
    let json = serde_json::to_string(&style).expect("serialise");
    let decoded: LineStyle = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(style, decoded);
}

#[test]
fn decorator_action_serde_round_trip() {
    for action in [
        DecoratorAction::CloseAnnotation,
        DecoratorAction::CreateTakeProfit,
        DecoratorAction::CreateStopLoss,
        DecoratorAction::CycleEntryType,
        DecoratorAction::EditQuantity,
        DecoratorAction::EditPrice,
        DecoratorAction::ToggleLocked,
        DecoratorAction::Submit,
        DecoratorAction::Save,
        DecoratorAction::Custom(42),
    ] {
        let json = serde_json::to_string(&action).expect("serialise");
        let decoded: DecoratorAction = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(action, decoded);
    }
}

#[test]
fn badge_serde_round_trip() {
    let badge = sample_badge();
    let json = serde_json::to_string(&badge).expect("serialise");
    let decoded: Badge = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(badge, decoded);
}

#[test]
fn button_serde_round_trip() {
    let button = sample_button();
    let json = serde_json::to_string(&button).expect("serialise");
    let decoded: Button = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(button, decoded);
}

#[test]
fn decorator_group_serde_round_trip() {
    let group = sample_group();
    let json = serde_json::to_string(&group).expect("serialise");
    let decoded: DecoratorGroup = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(group, decoded);
}

#[test]
fn decorator_group_nested_stack_serialises() {
    // Outer Row containing a nested Column stack (the ▲/▼ quick-action case).
    let inner_up = DecoratorItem {
        visibility: Visibility::OnGroupHover,
        action: Some(DecoratorAction::CreateTakeProfit),
        content: ItemContent::Button(Button {
            glyph: '▲',
            ..sample_button()
        }),
    };
    let inner_down = DecoratorItem {
        visibility: Visibility::OnGroupHover,
        action: Some(DecoratorAction::CreateStopLoss),
        content: ItemContent::Button(Button {
            glyph: '▼',
            ..sample_button()
        }),
    };
    let stack = DecoratorGroup {
        group_id: 2,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Column,
        gap: 1.0,
        items: smallvec![inner_up, inner_down],
    };
    let outer = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 2.0,
        items: smallvec![DecoratorItem {
            visibility: Visibility::OnGroupHover,
            action: None,
            content: ItemContent::Stack(Box::new(stack)),
        }],
    };
    let json = serde_json::to_string(&outer).expect("serialise");
    let decoded: DecoratorGroup = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(outer, decoded);
}

#[test]
fn badge_instance_shape_id_matches_enum() {
    // Stable contract with `midas-render/shaders/badge.wgsl`. Reordering
    // variants must not silently corrupt the shader switch — this test is
    // the canary. Adding a new variant means appending a discriminant.
    assert_eq!(BadgeShape::Rect.shape_id(), 0);
    assert_eq!(BadgeShape::Rounded { radius: 0.0 }.shape_id(), 1);
    assert_eq!(BadgeShape::Pill.shape_id(), 2);
    assert_eq!(BadgeShape::PointLeft { point_width: 0.0 }.shape_id(), 3);
    assert_eq!(BadgeShape::PointRight { point_width: 0.0 }.shape_id(), 4);
    assert_eq!(BadgeShape::DoublePoint { point_width: 0.0 }.shape_id(), 5);
    assert_eq!(BadgeShape::Chevron { point_width: 0.0 }.shape_id(), 6);
    assert_eq!(BadgeShape::Circle.shape_id(), 7);
}

#[test]
fn badge_shape_param_extracts_payloads() {
    assert_eq!(BadgeShape::Rect.shape_param(), 0.0);
    assert_eq!(BadgeShape::Rounded { radius: 4.5 }.shape_param(), 4.5);
    assert_eq!(BadgeShape::Pill.shape_param(), 0.0);
    assert_eq!(
        BadgeShape::PointLeft { point_width: 8.0 }.shape_param(),
        8.0
    );
    assert_eq!(BadgeShape::Chevron { point_width: 6.0 }.shape_param(), 6.0);
    assert_eq!(BadgeShape::Circle.shape_param(), 0.0);
}

// ===========================================================================
// Decorator compute + flex layout engine
// ===========================================================================

use crate::camera::Camera2D;
use crate::instances::GridLineInstance;
use crate::widget::compute::{ComputeContext, Viewport};
use crate::widget::decorator::compute::compute_decorator_group;
use crate::widget::hit_test::{HitZoneKind, ItemPath};
use crate::widget::theme::Theme;
use crate::widget::AnnotationId;
use midas_data::CandleBuffer;

/// Build a `ComputeContext` sized 1920x1080 with a simple 100ms / $30
/// camera window. Matches the shape used in `order_bracket::tests`.
fn make_test_camera() -> Camera2D {
    Camera2D {
        viewport_width: 1920,
        viewport_height: 1080,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 170.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    }
}

fn make_ctx<'a>(
    camera: &'a Camera2D,
    data: &'a CandleBuffer,
    theme: &'a Theme,
) -> ComputeContext<'a> {
    ComputeContext {
        camera,
        data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme,
        snap_fn: &|_| None,
        candle_duration_ms: 60_000.0,
        collapse_gaps: false,
        separator_y: camera.viewport_height as f32 * 0.80,
        dpi_scale: 1.0,
        hovered_annotation: None,
        hovered_decorator_groups: &[],
        selected_annotation: None,
        drag_ghost: None,
        pinned: false,
    }
}

/// Like [`make_ctx`] but with explicit hover-state fields. Passes empty
/// slices through the other fields.
fn make_ctx_with_hover<'a>(
    camera: &'a Camera2D,
    data: &'a CandleBuffer,
    theme: &'a Theme,
    hovered_annotation: Option<(AnnotationId, HitZoneKind)>,
    hovered_decorator_groups: &'a [(AnnotationId, u16)],
) -> ComputeContext<'a> {
    ComputeContext {
        camera,
        data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme,
        snap_fn: &|_| None,
        candle_duration_ms: 60_000.0,
        collapse_gaps: false,
        separator_y: camera.viewport_height as f32 * 0.80,
        dpi_scale: 1.0,
        hovered_annotation,
        hovered_decorator_groups,
        selected_annotation: None,
        drag_ghost: None,
        pinned: false,
    }
}

fn solid_badge(text: &str, width_hint: f32) -> Badge {
    Badge {
        shape: BadgeShape::Rect,
        fill: [0.2, 0.78, 0.35, 1.0],
        border: None,
        height: 18.0,
        padding: 2.0,
        segments: smallvec![BadgeSegment {
            text: text.into(),
            text_color: [1.0; 4],
            font_size: 11.0,
            min_width: Some(width_hint),
            fill_override: None,
            shape_override: None,
            action: None,
        }],
        divider_color: None,
    }
}

fn always_item(content: ItemContent) -> DecoratorItem {
    DecoratorItem {
        visibility: Visibility::Always,
        action: None,
        content,
    }
}

fn price_line() -> PriceLine {
    PriceLine {
        price: 185.0,
        extent: LineExtent::FullWidth,
        stroke: LineStroke {
            color: [0.2, 0.78, 0.35, 1.0],
            width: 1.5,
            style: LineStyle::Solid,
        },
    }
}

#[test]
fn hit_zone_kind_is_copy() {
    fn assert_copy<T: Copy>() {}
    assert_copy::<HitZoneKind>();
    assert_copy::<ItemPath>();
}

#[test]
fn decorator_row_lays_out_right_to_left_at_right_edge() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::RightEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![
            always_item(ItemContent::Badge(Box::new(solid_badge("A", 30.0)))),
            always_item(ItemContent::Badge(Box::new(solid_badge("B", 30.0)))),
        ],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);

    // Two badges -> two placeholder fills (no dividers).
    assert_eq!(out.fills.len(), 2);
    // First item in `items` order is the rightmost; second item is left
    // of the first.
    let first = out.fills[0].rect;
    let second = out.fills[1].rect;
    assert!(
        second[2] <= first[0],
        "second item should be entirely left of first: first={:?}, second={:?}",
        first,
        second,
    );
    // First item's right edge should touch the viewport right edge.
    assert!((first[2] - camera.viewport_width as f32).abs() < 0.01);
}

#[test]
fn decorator_gap_adds_spacing_between_items() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 7.0,
        items: smallvec![
            always_item(ItemContent::Badge(Box::new(solid_badge("A", 30.0)))),
            always_item(ItemContent::Badge(Box::new(solid_badge("B", 30.0)))),
        ],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);

    assert_eq!(out.fills.len(), 2);
    let a = out.fills[0].rect;
    let b = out.fills[1].rect;
    // Left-edge + Row packs forward; gap is exactly the distance between
    // a.right and b.left.
    assert!(
        (b[0] - a[2] - 7.0).abs() < 0.01,
        "gap should be 7.0 between rects; got {} - {} = {}",
        b[0],
        a[2],
        b[0] - a[2],
    );
}

#[test]
fn decorator_column_stacks_vertically() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Column,
        gap: 2.0,
        items: smallvec![
            always_item(ItemContent::Badge(Box::new(solid_badge("A", 30.0)))),
            always_item(ItemContent::Badge(Box::new(solid_badge("B", 30.0)))),
        ],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    assert_eq!(out.fills.len(), 2);
    let a = out.fills[0].rect;
    let b = out.fills[1].rect;
    assert!(
        b[1] >= a[3],
        "column: second should be below first; a.bottom={}, b.top={}",
        a[3],
        b[1],
    );
    assert!((b[1] - a[3] - 2.0).abs() < 0.01);
}

#[test]
fn decorator_spacer_consumes_width_emits_no_primitives() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![
            always_item(ItemContent::Badge(Box::new(solid_badge("A", 30.0)))),
            always_item(ItemContent::Spacer(12.0)),
            always_item(ItemContent::Badge(Box::new(solid_badge("B", 30.0)))),
        ],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    // Two badges -> exactly two fills. Spacer emitted nothing.
    assert_eq!(out.fills.len(), 2);
    assert_eq!(out.labels.len(), 2);
    assert_eq!(out.hit_zones.len(), 0);

    // And the spacer reserved space: b.left - a.right should be >= 12.
    let a = out.fills[0].rect;
    let b = out.fills[1].rect;
    assert!(
        (b[0] - a[2] - 12.0).abs() < 0.01,
        "spacer should reserve 12px between adjacent rects; got {}",
        b[0] - a[2],
    );
}

#[test]
fn decorator_badge_emits_segment_labels() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let badge = Badge {
        shape: BadgeShape::Rect,
        fill: [0.1, 0.1, 0.1, 1.0],
        border: None,
        height: 18.0,
        padding: 2.0,
        segments: smallvec![
            BadgeSegment {
                text: "L".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(20.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
            BadgeSegment {
                text: "100".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(30.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
            BadgeSegment {
                text: "185".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(30.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
        ],
        divider_color: None,
    };
    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_item(ItemContent::Badge(Box::new(badge)))],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    // Three segments -> three labels.
    assert_eq!(out.labels.len(), 3);
    let texts: Vec<&str> = out.labels.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, vec!["L", "100", "185"]);
}

#[test]
fn decorator_badge_divider_emits_vertical_rect_between_segments() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let badge = Badge {
        shape: BadgeShape::Rect,
        fill: [0.1, 0.1, 0.1, 1.0],
        border: None,
        height: 18.0,
        padding: 2.0,
        segments: smallvec![
            BadgeSegment {
                text: "A".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(20.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
            BadgeSegment {
                text: "B".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(20.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
            BadgeSegment {
                text: "C".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(20.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
        ],
        divider_color: Some([0.0, 0.0, 0.0, 0.4]),
    };
    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_item(ItemContent::Badge(Box::new(badge)))],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    // fills: 1 bounding-box + (3 - 1) dividers = 3 total.
    assert_eq!(out.fills.len(), 3);
    // Dividers are 1px wide, bounding rect is wider than 1.
    let dividers: Vec<&GridLineInstance> = out
        .fills
        .iter()
        .filter(|f| (f.rect[2] - f.rect[0] - 1.0).abs() < 0.01)
        .collect();
    assert_eq!(dividers.len(), 2);
}

#[test]
fn decorator_button_emits_one_badge_one_label_one_hit_zone() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let button = Button {
        shape: BadgeShape::Rounded { radius: 2.0 },
        fill: [0.9, 0.25, 0.25, 1.0],
        hover_fill: None,
        glyph: 'X',
        glyph_color: [1.0; 4],
        glyph_size: 11.0,
        size: [14.0, 14.0],
        border: None,
    };
    let group = DecoratorGroup {
        group_id: 7,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![DecoratorItem {
            visibility: Visibility::Always,
            action: Some(DecoratorAction::CloseAnnotation),
            content: ItemContent::Button(button),
        }],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(5), &ctx, 1.0);
    // Rounded shape → BadgeInstance, not a placeholder fill.
    assert_eq!(out.fills.len(), 0);
    assert_eq!(out.badges.len(), 1);
    assert_eq!(
        out.badges[0].shape_id,
        BadgeShape::Rounded { radius: 2.0 }.shape_id()
    );
    assert_eq!(out.labels.len(), 1);
    assert_eq!(out.hit_zones.len(), 1);

    let zone = &out.hit_zones[0];
    match zone.kind {
        HitZoneKind::Decorator {
            group_id,
            item_path,
            action,
        } => {
            assert_eq!(group_id, 7);
            assert_eq!(item_path.as_slice(), &[0u8]);
            assert_eq!(action, DecoratorAction::CloseAnnotation);
        }
        other => panic!("expected Decorator hit zone, got {:?}", other),
    }
}

#[test]
fn decorator_segment_with_action_emits_own_hit_zone() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let badge = Badge {
        shape: BadgeShape::Rect,
        fill: [0.1, 0.1, 0.1, 1.0],
        border: None,
        height: 18.0,
        padding: 2.0,
        segments: smallvec![
            BadgeSegment {
                text: "L".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(20.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
            BadgeSegment {
                text: "100".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(30.0),
                fill_override: None,
                shape_override: None,
                action: Some(DecoratorAction::EditQuantity),
            },
        ],
        divider_color: None,
    };
    let group = DecoratorGroup {
        group_id: 3,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_item(ItemContent::Badge(Box::new(badge)))],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    assert_eq!(out.hit_zones.len(), 1);
    let zone = &out.hit_zones[0];
    match zone.kind {
        HitZoneKind::Decorator {
            group_id,
            item_path,
            action,
        } => {
            assert_eq!(group_id, 3);
            assert_eq!(item_path.len(), 2);
            assert_eq!(item_path.as_slice(), &[0u8, 1u8]);
            assert_eq!(action, DecoratorAction::EditQuantity);
        }
        other => panic!("expected Decorator hit zone, got {:?}", other),
    }
}

#[test]
fn decorator_nested_stack_layout() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let inner = DecoratorGroup {
        group_id: 99, // ignored when used as a Stack child? no, group_id stays
        anchor: DecoratorAnchor::LeftEdge, // ignored for nested
        direction: FlexDirection::Column,
        gap: 1.0,
        items: smallvec![
            always_item(ItemContent::Badge(Box::new(solid_badge("U", 20.0)))),
            always_item(ItemContent::Badge(Box::new(solid_badge("D", 20.0)))),
        ],
    };
    let outer = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![
            always_item(ItemContent::Badge(Box::new(solid_badge("L", 30.0)))),
            always_item(ItemContent::Stack(Box::new(inner))),
        ],
    };

    let out = compute_decorator_group(&outer, &price_line(), AnnotationId(1), &ctx, 1.0);
    // 1 outer badge + 2 inner badges = 3 fills.
    assert_eq!(out.fills.len(), 3);
    // Inner stack items are column-stacked: the second's top should be
    // >= the first's bottom.
    let u = out.fills[1].rect;
    let d = out.fills[2].rect;
    assert!(
        d[1] >= u[3],
        "stack child layout: U.bottom={}, D.top={}",
        u[3],
        d[1],
    );
}

#[test]
fn decorator_item_path_breadcrumb_for_nested_stack_child() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    // Inner child is a button with an action -> emits a hit zone.
    let inner_button = DecoratorItem {
        visibility: Visibility::Always,
        action: Some(DecoratorAction::CreateTakeProfit),
        content: ItemContent::Button(Button {
            shape: BadgeShape::Rect,
            fill: [0.0, 1.0, 0.0, 1.0],
            hover_fill: None,
            glyph: '+',
            glyph_color: [1.0; 4],
            glyph_size: 10.0,
            size: [12.0, 12.0],
            border: None,
        }),
    };
    let inner = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Column,
        gap: 0.0,
        items: smallvec![inner_button],
    };
    // Outer has the stack at item index 2 (after two spacers so index
    // is non-trivial).
    let outer = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![
            always_item(ItemContent::Spacer(4.0)),
            always_item(ItemContent::Spacer(4.0)),
            always_item(ItemContent::Stack(Box::new(inner))),
        ],
    };

    let out = compute_decorator_group(&outer, &price_line(), AnnotationId(1), &ctx, 1.0);
    assert_eq!(out.hit_zones.len(), 1);
    let zone = &out.hit_zones[0];
    match zone.kind {
        HitZoneKind::Decorator { item_path, .. } => {
            // Path should be [stack_idx=2, child_idx=0], len=2.
            assert_eq!(item_path.len(), 2);
            assert_eq!(item_path.as_slice()[0], 2);
            assert_eq!(item_path.as_slice()[1], 0);
        }
        other => panic!("expected Decorator hit zone, got {:?}", other),
    }
}

#[test]
fn decorator_on_hover_items_skipped_in_slice_3() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![
            DecoratorItem {
                visibility: Visibility::OnLineHover,
                action: Some(DecoratorAction::CloseAnnotation),
                content: ItemContent::Badge(Box::new(solid_badge("A", 30.0))),
            },
            DecoratorItem {
                visibility: Visibility::OnGroupHover,
                action: Some(DecoratorAction::CloseAnnotation),
                content: ItemContent::Badge(Box::new(solid_badge("B", 30.0))),
            },
        ],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    assert_eq!(out.fills.len(), 0);
    assert_eq!(out.labels.len(), 0);
    assert_eq!(out.hit_zones.len(), 0);
}

#[test]
fn decorator_at_timestamp_anchor_uses_camera_time_to_x() {
    // Camera: 100_000ms window across 1920px -> 1ms = 0.0192px.
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    let ts: i64 = 50_000; // midpoint of the time window
    let expected_x = camera.time_to_x(ts as f64);

    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::AtTimestamp(ts),
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_item(ItemContent::Badge(Box::new(solid_badge(
            "A", 30.0
        ))))],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    assert_eq!(out.fills.len(), 1);
    let rect = out.fills[0].rect;
    // Row + AtTimestamp packs forward, so the badge's left edge sits
    // at expected_x.
    assert!(
        (rect[0] - expected_x).abs() < 0.01,
        "badge.left={} should equal time_to_x({})={}",
        rect[0],
        ts,
        expected_x,
    );
}

#[test]
fn compute_decorator_group_emits_badge_instance_for_point_left() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    // Build a PointLeft badge with a single plain segment.
    let badge = Badge {
        shape: BadgeShape::PointLeft { point_width: 6.0 },
        fill: [0.2, 0.78, 0.35, 1.0],
        border: Some(BadgeBorder {
            color: [1.0, 1.0, 1.0, 0.4],
            thickness: 1.5,
        }),
        height: 18.0,
        padding: 2.0,
        segments: smallvec![BadgeSegment {
            text: "P".into(),
            text_color: [1.0; 4],
            font_size: 11.0,
            min_width: Some(20.0),
            fill_override: None,
            shape_override: None,
            action: None,
        }],
        divider_color: None,
    };
    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_item(ItemContent::Badge(Box::new(badge)))],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    // Non-Rect shape → one BadgeInstance, zero placeholder fills for the body.
    assert_eq!(out.badges.len(), 1, "expected exactly one BadgeInstance");
    assert_eq!(
        out.fills.len(),
        0,
        "expected no placeholder fills for non-Rect body"
    );
    let badge_inst = &out.badges[0];
    assert_eq!(badge_inst.shape_id, 3, "PointLeft shape_id");
    assert!(
        (badge_inst.shape_param - 6.0).abs() < f32::EPSILON,
        "shape_param should be the point_width",
    );
    assert!(
        (badge_inst.border_thickness - 1.5).abs() < f32::EPSILON,
        "border_thickness propagated from BadgeBorder",
    );
    assert_eq!(badge_inst.fill, [0.2, 0.78, 0.35, 1.0]);
}

#[test]
fn compute_decorator_group_segment_shape_override_emits_extra_badge_instance() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx(&camera, &data, &theme);

    // Outer Pill badge with two segments; the second segment overrides
    // to a Circle with its own fill — this should emit an additional
    // BadgeInstance for the segment sub-rect on top of the outer pill.
    let badge = Badge {
        shape: BadgeShape::Pill,
        fill: [0.1, 0.1, 0.1, 1.0],
        border: None,
        height: 18.0,
        padding: 2.0,
        segments: smallvec![
            BadgeSegment {
                text: "TP".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(20.0),
                fill_override: None,
                shape_override: None,
                action: None,
            },
            BadgeSegment {
                text: "2".into(),
                text_color: [1.0; 4],
                font_size: 11.0,
                min_width: Some(18.0),
                fill_override: Some([0.0, 0.0, 0.0, 1.0]),
                shape_override: Some(BadgeShape::Circle),
                action: None,
            },
        ],
        divider_color: None,
    };
    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_item(ItemContent::Badge(Box::new(badge)))],
    };

    let out = compute_decorator_group(&group, &price_line(), AnnotationId(1), &ctx, 1.0);
    // One outer Pill + one segment Circle = 2 BadgeInstances.
    assert_eq!(out.badges.len(), 2);
    assert_eq!(out.fills.len(), 0);
    assert_eq!(out.badges[0].shape_id, 2, "Pill outer body");
    assert_eq!(out.badges[1].shape_id, 7, "Circle segment override");
    assert_eq!(out.badges[1].fill, [0.0, 0.0, 0.0, 1.0]);
}

// ===========================================================================
// Two-pass hover compute + `update()`-based recompute
// ===========================================================================

use crate::widget::compute::WidgetOutput;
use crate::widget::decorator::compute::{
    recompute_decorator_hit_zones, rect_contains, DecoratorGroupRef,
};

/// Build a mixed-visibility group: one `Always` badge, one
/// `OnLineHover` badge, one `OnGroupHover` button. Every item has an
/// action so emitted items are observable through `hit_zones`.
fn mixed_visibility_group(group_id: u16) -> DecoratorGroup {
    let always_badge = DecoratorItem {
        visibility: Visibility::Always,
        action: Some(DecoratorAction::EditQuantity),
        content: ItemContent::Badge(Box::new(solid_badge("A", 30.0))),
    };
    let on_line_badge = DecoratorItem {
        visibility: Visibility::OnLineHover,
        action: Some(DecoratorAction::ToggleLocked),
        content: ItemContent::Badge(Box::new(solid_badge("L", 30.0))),
    };
    let on_group_button = DecoratorItem {
        visibility: Visibility::OnGroupHover,
        action: Some(DecoratorAction::CloseAnnotation),
        content: ItemContent::Button(Button {
            shape: BadgeShape::Rect,
            fill: [0.9, 0.25, 0.25, 1.0],
            hover_fill: None,
            glyph: 'X',
            glyph_color: [1.0; 4],
            glyph_size: 11.0,
            size: [14.0, 14.0],
            border: None,
        }),
    };
    DecoratorGroup {
        group_id,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_badge, on_line_badge, on_group_button],
    }
}

/// Collect the `DecoratorAction`s present in a `WidgetOutput`'s hit
/// zones. Stable-ordered, so test assertions can compare slices.
fn actions_of(out: &WidgetOutput) -> Vec<DecoratorAction> {
    out.hit_zones
        .iter()
        .filter_map(|hz| match hz.kind {
            HitZoneKind::Decorator { action, .. } => Some(action),
            _ => None,
        })
        .collect()
}

#[test]
fn decorator_on_line_hover_emitted_when_parent_hovered() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(7);
    let ctx = make_ctx_with_hover(
        &camera,
        &data,
        &theme,
        Some((aid, HitZoneKind::LevelLine)),
        &[],
    );

    let group = mixed_visibility_group(1);
    let out = compute_decorator_group(&group, &price_line(), aid, &ctx, 1.0);
    let actions = actions_of(&out);

    // Line-hovered ⇒ Always + OnLineHover + OnGroupHover all emit.
    assert!(actions.contains(&DecoratorAction::EditQuantity));
    assert!(actions.contains(&DecoratorAction::ToggleLocked));
    assert!(actions.contains(&DecoratorAction::CloseAnnotation));
}

#[test]
fn decorator_on_group_hover_persists_when_group_expanded() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(7);
    // `hovered_annotation` is None — the line is no longer hovered.
    // `hovered_decorator_groups` contains the group id, simulating the
    // cursor having moved from the line onto a button on the previous
    // frame.
    let hovered = [(aid, 1u16)];
    let ctx = make_ctx_with_hover(&camera, &data, &theme, None, &hovered);

    let group = mixed_visibility_group(1);
    let out = compute_decorator_group(&group, &price_line(), aid, &ctx, 1.0);
    let actions = actions_of(&out);

    // Line is NOT hovered ⇒ OnLineHover is skipped, but OnGroupHover
    // still survives because the group is in the expanded set.
    assert!(actions.contains(&DecoratorAction::EditQuantity));
    assert!(
        !actions.contains(&DecoratorAction::ToggleLocked),
        "OnLineHover must be skipped when line is not hovered"
    );
    assert!(actions.contains(&DecoratorAction::CloseAnnotation));
}

#[test]
fn decorator_on_line_hover_skipped_when_not_hovered() {
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx_with_hover(&camera, &data, &theme, None, &[]);

    let group = mixed_visibility_group(1);
    let out = compute_decorator_group(&group, &price_line(), AnnotationId(7), &ctx, 1.0);
    let actions = actions_of(&out);

    assert!(actions.contains(&DecoratorAction::EditQuantity));
    assert!(!actions.contains(&DecoratorAction::ToggleLocked));
}

#[test]
fn decorator_on_group_hover_skipped_when_neither_line_nor_group_is_hovered() {
    // Positive control for the "cursor in dead space" frame.
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx_with_hover(&camera, &data, &theme, None, &[]);

    let group = mixed_visibility_group(1);
    let out = compute_decorator_group(&group, &price_line(), AnnotationId(7), &ctx, 1.0);
    let actions = actions_of(&out);

    assert!(!actions.contains(&DecoratorAction::ToggleLocked));
    assert!(!actions.contains(&DecoratorAction::CloseAnnotation));
    // Always item still visible.
    assert!(actions.contains(&DecoratorAction::EditQuantity));
}

#[test]
fn decorator_hover_set_update_adds_groups_for_hovered_line() {
    // Drive the two-step recompute loop at the `compute_decorator_group`
    // level: simulate the first `update()` tick where the line becomes
    // hovered. The helper produces a set of hit zones; the caller's
    // recompute adds the group id to the expanded set.
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(42);
    let ctx = make_ctx_with_hover(
        &camera,
        &data,
        &theme,
        Some((aid, HitZoneKind::LevelLine)),
        &[],
    );

    let group = mixed_visibility_group(3);
    let line = price_line();
    let refs = [DecoratorGroupRef {
        annotation_id: aid,
        group: &group,
        line: &line,
    }];
    let zones = recompute_decorator_hit_zones(&refs, &ctx);

    // Because the line is hovered, every hover-gated item is emitted
    // and present in `zones`. The app-layer recompute seeds the
    // expanded set with the group id when it sees a line hover, so the
    // existence of those zones is what makes this path testable.
    assert!(
        zones
            .iter()
            .any(|hz| matches!(hz.kind, HitZoneKind::Decorator { group_id: 3, .. })),
        "recompute should emit decorator zones for group 3"
    );
}

#[test]
fn decorator_hover_set_update_keeps_groups_with_cursor_over_item() {
    // Frame N-1 left the group expanded. Frame N's cursor has moved 10
    // pixels off the line and onto the close button. The recompute
    // must find a hit zone under the cursor so the group stays in the
    // expanded set.
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(42);
    // The previous frame's expansion is the ONLY truth source here —
    // `hovered_annotation` has already cleared on the line.
    let hovered_groups = [(aid, 3u16)];
    let ctx = make_ctx_with_hover(&camera, &data, &theme, None, &hovered_groups);

    let group = mixed_visibility_group(3);
    let line = price_line();
    let refs = [DecoratorGroupRef {
        annotation_id: aid,
        group: &group,
        line: &line,
    }];
    let zones = recompute_decorator_hit_zones(&refs, &ctx);

    // Pick the close button's zone and sample its center as the cursor
    // position. The hit-test must report containment ⇒ the app-layer
    // recompute would keep `(aid, 3)` in the expanded set.
    let close_zone = zones
        .iter()
        .find(|hz| {
            matches!(
                hz.kind,
                HitZoneKind::Decorator {
                    action: DecoratorAction::CloseAnnotation,
                    ..
                }
            )
        })
        .expect("OnGroupHover close button should emit because group is expanded");
    let cx = (close_zone.rect[0] + close_zone.rect[2]) * 0.5;
    let cy = (close_zone.rect[1] + close_zone.rect[3]) * 0.5;
    assert!(
        rect_contains(close_zone.rect, cx, cy),
        "cursor at the center of the close button should be contained"
    );
}

#[test]
fn decorator_hover_set_update_drops_groups_when_cursor_leaves_both_line_and_items() {
    // Neither line-hover nor expanded-group ⇒ the helper returns zero
    // zones for hover-gated items, which is how the app-layer recompute
    // knows to drop the group from the expanded set.
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let ctx = make_ctx_with_hover(&camera, &data, &theme, None, &[]);

    let group = mixed_visibility_group(3);
    let line = price_line();
    let refs = [DecoratorGroupRef {
        annotation_id: AnnotationId(42),
        group: &group,
        line: &line,
    }];
    let zones = recompute_decorator_hit_zones(&refs, &ctx);

    // Only the `Always` badge emits (EditQuantity). The cursor is
    // nowhere near it — pick a wildly out-of-bounds point and verify no
    // zone contains it, which is what the app-layer recompute uses to
    // clear the expanded set.
    let cursor_x = -10_000.0_f32;
    let cursor_y = -10_000.0_f32;
    assert!(
        zones
            .iter()
            .all(|hz| !rect_contains(hz.rect, cursor_x, cursor_y)),
        "no hit zone should contain an off-screen cursor"
    );
    // And critically, no `CloseAnnotation` zone exists at all, because
    // `OnGroupHover` was gated off.
    assert!(
        !zones.iter().any(|hz| matches!(
            hz.kind,
            HitZoneKind::Decorator {
                action: DecoratorAction::CloseAnnotation,
                ..
            }
        )),
        "OnGroupHover button must not emit when group is not expanded"
    );
}

#[test]
fn decorator_first_frame_hover_no_flicker() {
    // The L2 regression test from 05-interaction.md "First-frame hover
    // edge case (walkthrough)".
    //
    // Frame N-1: line hovered, close button visible. The expanded set
    // contains `(aid, group_id)`.
    // Frame N: cursor has moved 10px onto the button. The line itself
    // is no longer hovered. The recompute must:
    //   - still emit the close button zone (because the group is
    //     expanded from the previous frame), and
    //   - the button's hit zone must contain the new cursor position,
    //     so the app-layer recompute keeps the group in the set and
    //     `hovered_annotation` flips from `LevelLine` to
    //     `Decorator { .. }` via the decorator-fallback path.
    let camera = make_test_camera();
    let data = CandleBuffer::new();
    let theme = Theme::default();
    let aid = AnnotationId(7);

    // Frame N-1 ctx: the line IS hovered. Precompute the button rect
    // so we know where to "move" the cursor on frame N.
    //
    // The group MUST omit `OnLineHover` items for the invariant to
    // hold: per 05-interaction.md, `OnLineHover` is reserved for items
    // that *should* disappear on cursor-off-line (cosmetic drag
    // affordances), so the layout must not depend on them persisting.
    // Persistent click targets use `OnGroupHover`, which is what this
    // test reproduces.
    let ctx_prev = make_ctx_with_hover(
        &camera,
        &data,
        &theme,
        Some((aid, HitZoneKind::LevelLine)),
        &[],
    );
    let always_badge = DecoratorItem {
        visibility: Visibility::Always,
        action: Some(DecoratorAction::EditQuantity),
        content: ItemContent::Badge(Box::new(solid_badge("A", 30.0))),
    };
    let on_group_button = DecoratorItem {
        visibility: Visibility::OnGroupHover,
        action: Some(DecoratorAction::CloseAnnotation),
        content: ItemContent::Button(Button {
            shape: BadgeShape::Rect,
            fill: [0.9, 0.25, 0.25, 1.0],
            hover_fill: None,
            glyph: 'X',
            glyph_color: [1.0; 4],
            glyph_size: 11.0,
            size: [14.0, 14.0],
            border: None,
        }),
    };
    let group = DecoratorGroup {
        group_id: 1,
        anchor: DecoratorAnchor::LeftEdge,
        direction: FlexDirection::Row,
        gap: 0.0,
        items: smallvec![always_badge, on_group_button],
    };
    let line = price_line();
    let out_prev = compute_decorator_group(&group, &line, aid, &ctx_prev, 1.0);
    let prev_close = out_prev
        .hit_zones
        .iter()
        .find(|hz| {
            matches!(
                hz.kind,
                HitZoneKind::Decorator {
                    action: DecoratorAction::CloseAnnotation,
                    ..
                }
            )
        })
        .expect("frame N-1 must emit the close button");
    let cursor_x = (prev_close.rect[0] + prev_close.rect[2]) * 0.5;
    let cursor_y = (prev_close.rect[1] + prev_close.rect[3]) * 0.5;

    // Frame N ctx: `hovered_annotation` has cleared (cursor left the
    // line), but the expanded set carries `(aid, 1)` over from N-1.
    let hovered_groups = [(aid, 1u16)];
    let ctx_next = make_ctx_with_hover(&camera, &data, &theme, None, &hovered_groups);
    let refs = [DecoratorGroupRef {
        annotation_id: aid,
        group: &group,
        line: &line,
    }];
    let zones_next = recompute_decorator_hit_zones(&refs, &ctx_next);

    // Invariant 1: the close button's zone is STILL in the output.
    let close_next = zones_next
        .iter()
        .find(|hz| {
            matches!(
                hz.kind,
                HitZoneKind::Decorator {
                    action: DecoratorAction::CloseAnnotation,
                    ..
                }
            )
        })
        .expect("frame N must still emit the close button because the group is expanded");

    // Invariant 2: the (unchanged) close button still contains the
    // cursor position, so the app-layer recompute keeps the group in
    // the set.
    assert!(
        rect_contains(close_next.rect, cursor_x, cursor_y),
        "close button rect {:?} must contain cursor ({cursor_x}, {cursor_y})",
        close_next.rect
    );

    // Invariant 3: the fallback path produces a
    // `HitZoneKind::Decorator` for the same annotation id — this is
    // what `hovered_annotation` flips to in `chart_widget.rs::update()`.
    match close_next.kind {
        HitZoneKind::Decorator { group_id, .. } => {
            assert_eq!(
                group_id, 1,
                "group_id should round-trip through the recompute"
            );
            assert_eq!(
                close_next.annotation_id, aid,
                "fallback must point at the same annotation"
            );
        }
        other => panic!("expected Decorator hit zone, got {other:?}"),
    }
}
