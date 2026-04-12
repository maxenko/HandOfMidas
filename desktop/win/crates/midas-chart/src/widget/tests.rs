use super::*;
use crate::levels::LevelIcon;
use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
use smallvec::smallvec;

fn make_annotation(id: u64, price: f64) -> Annotation {
    Annotation {
        id: AnnotationId(id),
        kind: AnnotationKind::Level(HorizontalLevel {
            id,
            line: PriceLine {
                price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [0.0, 0.7, 1.0, 0.9],
                    width: 1.0,
                    style: LineStyle::default(),
                },
            },
            label: None,
            icon: LevelIcon::None,
        }),
        presence: Presence::Active,
        visible_timeframes: None,
        locked: false,
        created_at: 0,
        modified_at: 0,
    }
}

#[test]
fn annotation_id_sentinel() {
    assert!(!AnnotationId::NONE.is_valid());
    assert!(AnnotationId(1).is_valid());
    assert!(AnnotationId(42).is_valid());
}

#[test]
fn annotation_id_display() {
    assert_eq!(AnnotationId(42).to_string(), "ann#42");
    assert_eq!(AnnotationId::NONE.to_string(), "ann#0");
}

#[test]
fn presence_alpha_values() {
    assert_eq!(Presence::Active.alpha(), 1.0);
    assert_eq!(Presence::Ghost.alpha(), 0.4);
    assert_eq!(Presence::Hidden.alpha(), 0.0);
}

#[test]
fn presence_visibility_and_interaction() {
    assert!(Presence::Active.is_visible());
    assert!(Presence::Active.is_interactive());
    assert!(Presence::Active.is_hit_testable());

    assert!(Presence::Ghost.is_visible());
    assert!(!Presence::Ghost.is_interactive());
    assert!(!Presence::Ghost.is_hit_testable());

    assert!(!Presence::Hidden.is_visible());
    assert!(!Presence::Hidden.is_interactive());
    assert!(!Presence::Hidden.is_hit_testable());
}

#[test]
fn presence_cycle() {
    assert_eq!(Presence::Active.cycle(), Presence::Ghost);
    assert_eq!(Presence::Ghost.cycle(), Presence::Hidden);
    assert_eq!(Presence::Hidden.cycle(), Presence::Active);
}

#[test]
fn annotation_should_render_on_all_timeframes_by_default() {
    let ann = make_annotation(1, 185.0);
    assert!(ann.should_render_on(Timeframe::M5));
    assert!(ann.should_render_on(Timeframe::D1));
    assert!(ann.should_render_on(Timeframe::H1));
}

#[test]
fn annotation_should_render_respects_timeframe_filter() {
    let mut ann = make_annotation(1, 185.0);
    ann.visible_timeframes = Some(vec![Timeframe::M5, Timeframe::M15]);

    assert!(ann.should_render_on(Timeframe::M5));
    assert!(ann.should_render_on(Timeframe::M15));
    assert!(!ann.should_render_on(Timeframe::D1));
    assert!(!ann.should_render_on(Timeframe::H1));
}

#[test]
fn annotation_hidden_never_renders() {
    let mut ann = make_annotation(1, 185.0);
    ann.presence = Presence::Hidden;
    assert!(!ann.should_render_on(Timeframe::M5));
    assert!(!ann.should_render_on(Timeframe::D1));
}

#[test]
fn annotation_ghost_renders_but_not_interactive() {
    let mut ann = make_annotation(1, 185.0);
    ann.presence = Presence::Ghost;
    assert!(ann.should_render_on(Timeframe::D1));
    assert!(!ann.is_interactive_on(Timeframe::D1));
    assert!(!ann.is_draggable_on(Timeframe::D1));
}

#[test]
fn annotation_locked_not_draggable() {
    let mut ann = make_annotation(1, 185.0);
    ann.locked = true;
    assert!(ann.should_render_on(Timeframe::D1));
    assert!(ann.is_interactive_on(Timeframe::D1));
    assert!(!ann.is_draggable_on(Timeframe::D1));
}

#[test]
fn annotation_serde_round_trip() {
    let ann = make_annotation(42, 175.50);
    let json = serde_json::to_string(&ann).expect("serialize");
    let decoded: Annotation = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.id, AnnotationId(42));
    assert!(decoded.visible_timeframes.is_none());
    assert!(!decoded.locked);

    // Verify the level data survived.
    match &decoded.kind {
        AnnotationKind::Level(level) => {
            assert!((level.line.price - 175.50).abs() < f64::EPSILON);
            assert_eq!(level.line.stroke.width, 1.0);
        }
        _ => panic!("expected Level variant"),
    }
}

#[test]
fn horizontal_level_serde_round_trip() {
    let level = HorizontalLevel {
        id: 42,
        line: PriceLine {
            price: 200.0,
            extent: LineExtent::FullWidth,
            stroke: LineStroke {
                color: [1.0, 0.0, 0.0, 1.0],
                width: 2.0,
                style: LineStyle::Pattern(smallvec![8.0, 4.0]),
            },
        },
        label: Some("Resistance".into()),
        icon: LevelIcon::Star,
    };
    let json = serde_json::to_string(&level).expect("serialize");
    let decoded: HorizontalLevel = serde_json::from_str(&json).expect("deserialize");
    assert!((decoded.line.price - 200.0).abs() < f64::EPSILON);
    assert_eq!(decoded.label.as_deref(), Some("Resistance"));
    assert_eq!(decoded.icon, LevelIcon::Star);
}

#[test]
fn widget_output_apply_alpha() {
    use crate::instances::GridLineInstance;

    let mut output = WidgetOutput {
        fills: vec![GridLineInstance {
            rect: [0.0, 0.0, 100.0, 1.0],
            color: [1.0, 0.0, 0.0, 1.0],
        }],
        lines: vec![GridLineInstance {
            rect: [0.0, 50.0, 100.0, 51.0],
            color: [0.0, 1.0, 0.0, 0.8],
        }],
        markers: vec![],
        badges: vec![],
        labels: vec![WidgetLabel {
            text: "Test".into(),
            screen_x: 10.0,
            screen_y: 20.0,
            bg_color: [0.0, 0.0, 0.0, 1.0],
            text_color: [1.0, 1.0, 1.0, 1.0],
            font_size: 11.0,
            anchor: LabelAnchor::TopLeft,
        }],
        hit_zones: vec![],
    };

    output.apply_alpha(0.4);
    assert!((output.fills[0].color[3] - 0.4).abs() < f32::EPSILON);
    assert!((output.lines[0].color[3] - 0.32).abs() < f32::EPSILON);
    assert!((output.labels[0].bg_color[3] - 0.4).abs() < f32::EPSILON);
    assert!((output.labels[0].text_color[3] - 0.4).abs() < f32::EPSILON);
}

#[test]
fn widget_output_merge() {
    use crate::instances::GridLineInstance;

    let mut a = WidgetOutput::empty();
    a.fills.push(GridLineInstance {
        rect: [0.0, 0.0, 100.0, 1.0],
        color: [1.0, 0.0, 0.0, 1.0],
    });

    let mut b = WidgetOutput::empty();
    b.lines.push(GridLineInstance {
        rect: [0.0, 50.0, 100.0, 51.0],
        color: [0.0, 1.0, 0.0, 0.8],
    });

    a.merge(b);
    assert_eq!(a.fills.len(), 1);
    assert_eq!(a.lines.len(), 1);
    assert_eq!(a.instance_count(), 2);
}

#[test]
fn bounding_box_contains() {
    let bb = BoundingBox {
        left: 10.0,
        top: 20.0,
        right: 100.0,
        bottom: 80.0,
    };
    assert!(bb.contains(Point { x: 50.0, y: 50.0 }));
    assert!(bb.contains(Point { x: 10.0, y: 20.0 }));
    assert!(!bb.contains(Point { x: 5.0, y: 50.0 }));
    assert!(!bb.contains(Point { x: 50.0, y: 90.0 }));
}

#[test]
fn bounding_box_expand() {
    let bb = BoundingBox {
        left: 10.0,
        top: 20.0,
        right: 100.0,
        bottom: 80.0,
    };
    let expanded = bb.expand(5.0);
    assert_eq!(expanded.left, 5.0);
    assert_eq!(expanded.top, 15.0);
    assert_eq!(expanded.right, 105.0);
    assert_eq!(expanded.bottom, 85.0);
}

// ── Slice 7: compute_level visual parity tests ─────────────────────

/// Build a minimal `ComputeContext` targeting a 400x300 viewport with a
/// simple $100→$200 / 0→100k ms camera window. Only used by the Slice 7
/// `compute_level` parity tests below — smaller footprint than the
/// decorator-tests helper so the empty `CandleBuffer` isn't needed.
fn make_slice7_camera() -> crate::camera::Camera2D {
    crate::camera::Camera2D {
        viewport_width: 400,
        viewport_height: 300,
        time_start: 0.0,
        time_end: 100_000.0,
        price_low: 100.0,
        price_high: 200.0,
        dpi_scale: 1.0,
    }
}

#[test]
fn compute_level_hit_zones_include_line_plus_decorators() {
    use crate::widget::compute::{ComputeContext, Viewport};
    use crate::widget::hit_test::HitZoneKind;
    use crate::widget::theme::Theme;

    let camera = make_slice7_camera();
    let data = midas_data::CandleBuffer::new();
    let theme = Theme::default();
    let ctx = ComputeContext {
        camera: &camera,
        data: &data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme: &theme,
        snap_fn: &|_| None,
        candle_duration_ms: 60_000.0,
        collapse_gaps: false,
        separator_y: 240.0,
        dpi_scale: 1.0,
        hovered_annotation: None,
        hovered_decorator_groups: &[],
        selected_annotation: None,
        drag_ghost: None,
        pinned: false,
    };

    let level = HorizontalLevel {
        id: 42,
        line: PriceLine {
            price: 150.0,
            extent: LineExtent::default(),
            stroke: LineStroke {
                color: [0.2, 0.8, 0.4, 1.0],
                width: 1.5,
                style: LineStyle::Solid,
            },
        },
        label: Some("Mid".into()),
        icon: LevelIcon::Star,
    };
    let out = compute_level(&level, AnnotationId(42), &ctx, 1.0, false);

    // The line-level hit zone must exist exactly once.
    let line_zones = out
        .hit_zones
        .iter()
        .filter(|z| matches!(z.kind, HitZoneKind::LevelLine))
        .count();
    assert_eq!(
        line_zones, 1,
        "compute_level should emit exactly one LevelLine hit zone"
    );

    // At minimum the right-edge price badge is emitted as a rect fill,
    // and the left-edge icon+label badge is emitted as another fill.
    // Rect badges route through `WidgetOutput.fills` per Slice 4.
    assert!(
        !out.fills.is_empty(),
        "decorator badges should emit at least one fill"
    );
    // And at least one label per badge segment (price badge + left badge).
    assert!(
        out.labels.len() >= 2,
        "expected >= 2 decorator segment labels, got {}",
        out.labels.len()
    );
    // And at least one line primitive for the actual stroke.
    assert!(
        !out.lines.is_empty(),
        "compute_price_line_geometry should emit the line stroke"
    );
}

#[test]
fn compute_level_visual_parity_with_pre_refactor() {
    use crate::widget::compute::{ComputeContext, Viewport};
    use crate::widget::theme::Theme;

    let camera = make_slice7_camera();
    let data = midas_data::CandleBuffer::new();
    let theme = Theme::default();
    let ctx = ComputeContext {
        camera: &camera,
        data: &data,
        viewport: Viewport {
            width: camera.viewport_width,
            height: camera.viewport_height,
        },
        theme: &theme,
        snap_fn: &|_| None,
        candle_duration_ms: 60_000.0,
        collapse_gaps: false,
        separator_y: 240.0,
        dpi_scale: 1.0,
        hovered_annotation: None,
        hovered_decorator_groups: &[],
        selected_annotation: None,
        drag_ghost: None,
        pinned: false,
    };

    // Bare level (no label, no icon, unlocked) → group 0 only.
    let level = HorizontalLevel {
        id: 1,
        line: PriceLine {
            price: 150.0,
            extent: LineExtent::default(),
            stroke: LineStroke {
                color: [1.0, 0.0, 0.0, 1.0],
                width: 1.0,
                style: LineStyle::Solid,
            },
        },
        label: None,
        icon: LevelIcon::None,
    };
    let out = compute_level(&level, AnnotationId(1), &ctx, 1.0, false);

    // Baseline primitive counts for the "plain level" shape:
    //  - lines:    1 (single solid segment spanning the viewport).
    //  - fills:    1 (right-edge price badge, emitted via Rect fill path).
    //  - labels:   1 (price segment text).
    //  - hit_zones:1 (LevelLine — the decorator's `action: None` omits
    //                 a decorator hit zone, so only the line zone exists).
    assert_eq!(out.lines.len(), 1, "lines");
    assert_eq!(out.fills.len(), 1, "fills (price badge)");
    assert_eq!(out.labels.len(), 1, "labels (price text)");
    assert_eq!(out.hit_zones.len(), 1, "hit_zones (LevelLine only)");
}
