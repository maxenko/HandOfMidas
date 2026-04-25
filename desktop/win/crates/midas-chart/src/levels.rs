//! Horizontal price levels.
//!
//! Slice A1 of `plan/arch-review-fixes/01-group-a-types-extraction.md`
//! moved the data types (`HorizontalLevel`, `LevelIcon`, V1/V2
//! deserialize migration, `price_step_for`) into the leaf crate
//! `midas-annotation-types`. This file is now a shim that re-exports
//! those types and hosts the chart-only decorator-emission helper as
//! the [`HorizontalLevelExt`] extension trait — Rust's orphan rule
//! forbids adding inherent methods to a type defined in another crate,
//! so `level.to_decorators(locked)` is reached via a trait import.
//!
//! ## Lock semantics
//!
//! Per the upstream plan, `locked` conceptually lives on the
//! `Annotation` wrapper at `widget/mod.rs` rather than on the inner
//! `HorizontalLevel`. That is true for every level that flows through
//! the `AnnotationStore`. The app's pre-decorator `LevelStore` path
//! still stores bare levels outside of an `Annotation` wrapper (it
//! predates the unified widget system), so a sibling
//! `StoredLevel { level, locked }` wrapper type lives in
//! `midas-app/src/level_store/mod.rs` and owns the lock flag for that
//! path. `to_decorators(locked)` takes `locked` as an explicit argument
//! so both paths can drive the same decorator emission from a
//! wrapper-side flag.

use crate::widget::decorator::{
    Badge, BadgeSegment, BadgeShape, DecoratorAnchor, DecoratorGroup, DecoratorItem, FlexDirection,
    ItemContent, Visibility,
};
use smallvec::smallvec;

// ── Data types: re-exported from the new leaf crate. ──────────────
// `HorizontalLevel`, `LevelIcon`, `price_step_for`, and the V1/V2
// `Deserialize` impl all live in `midas-annotation-types::levels`.
//
// A1b added `#[deprecated]` after consumer-side migration so future
// imports via `midas_chart::levels::*` (or the top-level
// `midas_chart::HorizontalLevel`/`midas_chart::LevelIcon` re-exports
// in `lib.rs`) emit warnings caught by `cargo clippy -D warnings`.
#[deprecated(
    note = "import from midas_annotation_types directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_annotation_types::levels::{price_step_for, HorizontalLevel, LevelIcon};

/// Extension trait carrying chart-side decorator emission for
/// `HorizontalLevel`.
///
/// Lives here (not in `midas-annotation-types`) because the body
/// depends on chart-only `Badge`/`DecoratorGroup`/`PRICELINE_WIDTH`
/// types and on `crate::color::contrast_text_color`. The orphan rule
/// forbids `impl HorizontalLevel { … }` blocks in this crate now that
/// `HorizontalLevel` has moved, so existing `level.to_decorators(…)`
/// call sites import this trait to retain the method-call form.
pub trait HorizontalLevelExt {
    /// Build the standard level decorator set.
    ///
    /// - **Group 0**: right-edge price badge (always emitted).
    /// - **Group 1**: left-edge packed row holding the optional lock
    ///   badge followed by the optional label/icon badge. Emitted only
    ///   when at least one of those items is present. Packing both
    ///   items into a single group lets the flex layout lay them out
    ///   side-by-side on the same row so their rects and hit zones
    ///   never overlap.
    ///
    /// `locked` is sourced from the wrapper (`Annotation.locked` or
    /// the level-store-side `StoredLevel.locked`), not from the level
    /// itself.
    fn to_decorators(&self, locked: bool) -> Vec<DecoratorGroup>;
}

impl HorizontalLevelExt for HorizontalLevel {
    fn to_decorators(&self, locked: bool) -> Vec<DecoratorGroup> {
        let mut groups: Vec<DecoratorGroup> = Vec::new();
        let line_color = self.line.stroke.color;

        // Group 0: chart-area right-edge price badge.
        // Pointed-left flag in the level's own color, opaque, with the
        // price text drawn in black/white — whichever has the higher
        // contrast against the fill. `AtChartRightEdge` anchors the
        // triangle tip on the vertical priceline border and lets the
        // body extend rightward into the axis area.
        // `action: None` in Slice 7 — clicks fall through to the
        // existing `HitZoneKind::LevelLine` drag hit zone emitted by
        // `compute_price_line_geometry`.
        let badge_fill = [line_color[0], line_color[1], line_color[2], 1.0];
        groups.push(DecoratorGroup {
            group_id: 0,
            // Shift the anchor left by the `PointLeft` point_width so
            // the triangle base (body left edge) aligns with the
            // priceline and the tip protrudes into the chart.
            anchor: DecoratorAnchor::AtChartRightEdge { pointer_inset: 8.0 },
            direction: FlexDirection::Row,
            gap: 0.0,
            items: smallvec![DecoratorItem {
                visibility: Visibility::Always,
                action: None,
                content: ItemContent::Badge(Box::new(Badge {
                    shape: BadgeShape::PointLeft { point_width: 8.0 },
                    fill: badge_fill,
                    border: None,
                    height: 18.0,
                    padding: 6.0,
                    segments: smallvec![BadgeSegment {
                        text: format!("{:.2}", self.line.price),
                        text_color: crate::color::contrast_text_color(badge_fill),
                        font_size: 11.0,
                        // Force the badge body (= badge_width - point_width) to
                        // span from the priceline to the viewport right edge
                        // — no gap on the right. With outer padding 6 on each
                        // side and point_width 8:
                        //   body_width = 2*6 + seg_w - 8 = seg_w + 4.
                        // We want body_width >= PRICELINE_WIDTH (60), so
                        //   seg_w >= 56. Longer prices grow past this
                        //   naturally and overflow slightly past vp_w.
                        min_width: Some(crate::compute::PRICELINE_WIDTH - 4.0),
                        fill_override: None,
                        shape_override: None,
                        action: None,
                    }],
                    divider_color: None,
                })),
            }],
        });

        // Group 1: left-edge packed row. Holds (in reading order) the
        // lock badge followed by the label/icon badge. Either item may
        // be absent; the group is only emitted when at least one is
        // present so unadorned levels produce no left-side decorators.
        let has_label = self.label.as_deref().is_some_and(|s| !s.is_empty());
        let has_icon = self.icon != LevelIcon::None;
        let has_label_or_icon = has_label || has_icon;
        if locked || has_label_or_icon {
            let mut items: smallvec::SmallVec<[DecoratorItem; 4]> = smallvec![];

            // Left-side badges mirror the right-side price badge's
            // palette: opaque fill in the level's own color with
            // black/white contrast text. `Rounded { radius: 0.0 }`
            // renders identically to `Rect` but routes through the SDF
            // badge pipeline instead of `fills`, which draws after the
            // grid/line pipeline — the line no longer peeks through
            // the badge.
            let badge_fill = [line_color[0], line_color[1], line_color[2], 1.0];
            let badge_text_color = crate::color::contrast_text_color(badge_fill);

            if locked {
                items.push(DecoratorItem {
                    visibility: Visibility::Always,
                    action: None,
                    content: ItemContent::Badge(Box::new(Badge {
                        shape: BadgeShape::Rounded { radius: 0.0 },
                        fill: badge_fill,
                        border: None,
                        height: 18.0,
                        padding: 6.0,
                        segments: smallvec![BadgeSegment {
                            // U+1F512 lock glyph may not ship with the
                            // standard font — use the plain text "LOCK"
                            // tag which every font renders.
                            text: "LOCK".to_owned(),
                            text_color: badge_text_color,
                            font_size: 10.0,
                            min_width: None,
                            fill_override: None,
                            shape_override: None,
                            action: None,
                        }],
                        divider_color: None,
                    })),
                });
            }

            if has_label_or_icon {
                let icon_text = match (self.label.as_deref(), self.icon.as_char()) {
                    (Some(lbl), Some(icon_ch)) if !lbl.is_empty() => format!("{icon_ch} {lbl}"),
                    (Some(lbl), None) if !lbl.is_empty() => lbl.to_owned(),
                    (_, Some(icon_ch)) => icon_ch.to_string(),
                    _ => String::new(),
                };
                items.push(DecoratorItem {
                    visibility: Visibility::Always,
                    action: None,
                    content: ItemContent::Badge(Box::new(Badge {
                        shape: BadgeShape::Rounded { radius: 0.0 },
                        fill: badge_fill,
                        border: None,
                        height: 20.0,
                        padding: 6.0,
                        segments: smallvec![BadgeSegment {
                            text: icon_text,
                            text_color: badge_text_color,
                            font_size: 14.0,
                            min_width: None,
                            fill_override: None,
                            shape_override: None,
                            action: None,
                        }],
                        divider_color: None,
                    })),
                });
            }

            groups.push(DecoratorGroup {
                group_id: 1,
                anchor: DecoratorAnchor::LeftEdge,
                direction: FlexDirection::Row,
                gap: 4.0,
                items,
            });
        }

        groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
    use crate::widget::LineStyle;

    fn make_level(id: u64, price: f64) -> HorizontalLevel {
        HorizontalLevel {
            id,
            line: PriceLine {
                price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [1.0, 0.0, 0.0, 1.0],
                    width: 1.0,
                    style: LineStyle::Solid,
                },
            },
            label: None,
            icon: LevelIcon::None,
        }
    }

    #[test]
    fn level_clone_and_debug() {
        let level = make_level(1, 150.0);
        let cloned = level.clone();
        assert_eq!(cloned.id, 1);
        assert_eq!(cloned.line.price, 150.0);
        let _ = format!("{:?}", cloned);
    }

    #[test]
    fn level_icon_round_trip() {
        for icon in LevelIcon::all() {
            let id = icon.to_str_id();
            let restored = LevelIcon::from_str_id(id);
            assert_eq!(&restored, icon);
        }
    }

    #[test]
    fn price_step_for_various_prices() {
        let (c, f) = price_step_for(250.0);
        assert_eq!(c, 0.05);
        assert_eq!(f, 0.05);

        let (c, f) = price_step_for(50.0);
        assert_eq!(c, 0.01);
        assert_eq!(f, 0.01);

        let (c, f) = price_step_for(199.99);
        assert_eq!(c, 0.01);
        assert_eq!(f, 0.01);

        let (c, f) = price_step_for(200.0);
        assert_eq!(c, 0.05);
        assert_eq!(f, 0.05);
    }

    #[test]
    fn level_v2_serde_round_trip() {
        let mut level = make_level(42, 175.5);
        level.label = Some("Resistance".into());
        level.icon = LevelIcon::ArrowUp;
        let json = serde_json::to_string(&level).expect("serialize");
        let decoded: HorizontalLevel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.id, 42);
        assert!((decoded.line.price - 175.5).abs() < f64::EPSILON);
        assert_eq!(decoded.line.stroke.width, 1.0);
        assert_eq!(decoded.label.as_deref(), Some("Resistance"));
        assert_eq!(decoded.icon, LevelIcon::ArrowUp);
    }

    #[test]
    fn horizontal_level_config_v1_migrates_to_new_shape() {
        // Legacy flat v1 shape (pre-Slice-7 persistence format).
        let json = r#"{
            "id": 7,
            "price": 189.42,
            "color": [0.2, 0.6, 1.0, 0.9],
            "line_width": 2.0,
            "style": "Solid",
            "label": "Support",
            "icon": "Star",
            "extend": "FullWidth",
            "locked": false
        }"#;
        let decoded: HorizontalLevel = serde_json::from_str(json).expect("v1 -> v2 migration");
        assert_eq!(decoded.id, 7);
        assert!((decoded.line.price - 189.42).abs() < f64::EPSILON);
        assert_eq!(decoded.line.stroke.color, [0.2, 0.6, 1.0, 0.9]);
        assert_eq!(decoded.line.stroke.width, 2.0);
        assert_eq!(decoded.line.extent, LineExtent::FullWidth);
        assert_eq!(decoded.label.as_deref(), Some("Support"));
        assert_eq!(decoded.icon, LevelIcon::Star);
        assert!(matches!(decoded.line.stroke.style, LineStyle::Solid));
    }

    #[test]
    fn horizontal_level_to_decorators_right_badge_shows_price() {
        let mut level = make_level(1, 123.45);
        level.line.stroke.color = [1.0, 0.0, 0.0, 1.0];
        let groups = level.to_decorators(false);
        assert!(!groups.is_empty(), "group 0 must always exist");
        let g0 = &groups[0];
        assert_eq!(g0.group_id, 0);
        assert!(matches!(
            g0.anchor,
            DecoratorAnchor::AtChartRightEdge { .. }
        ));
        let item = &g0.items[0];
        match &item.content {
            ItemContent::Badge(b) => {
                assert_eq!(b.segments.len(), 1);
                assert_eq!(b.segments[0].text, "123.45");
            }
            _ => panic!("expected badge in group 0"),
        }
    }

    #[test]
    fn horizontal_level_to_decorators_left_badge_shows_label() {
        let mut level = make_level(1, 100.0);
        level.label = Some("Support".into());
        let groups = level.to_decorators(false);
        assert_eq!(groups.len(), 2, "expected groups 0 + 1");
        let g1 = &groups[1];
        assert_eq!(g1.group_id, 1);
        assert!(matches!(g1.anchor, DecoratorAnchor::LeftEdge));
        assert_eq!(g1.items.len(), 1, "unlocked label-only row has 1 item");
        match &g1.items[0].content {
            ItemContent::Badge(b) => {
                assert!(
                    b.segments[0].text.contains("Support"),
                    "label badge text missing: {}",
                    b.segments[0].text
                );
            }
            _ => panic!("expected badge in group 1"),
        }

        // Without label, icon, or lock, group 1 is omitted.
        let level = make_level(2, 100.0);
        let groups = level.to_decorators(false);
        assert_eq!(groups.len(), 1, "no label, icon, or lock: only group 0");
    }

    #[test]
    fn horizontal_level_to_decorators_icon_only_no_label_group() {
        let mut level = make_level(1, 100.0);
        level.icon = LevelIcon::Star;
        let groups = level.to_decorators(false);
        assert_eq!(groups.len(), 2, "icon-only should still produce group 1");
        assert_eq!(groups[1].group_id, 1);
    }

    #[test]
    fn horizontal_level_to_decorators_locked_only_emits_single_item_row() {
        // Locked + no label + no icon: group 1 should carry exactly the
        // lock badge, no extra label item.
        let level = make_level(1, 100.0);
        let unlocked = level.to_decorators(false);
        assert_eq!(unlocked.len(), 1, "no lock group when unlocked");

        let groups = level.to_decorators(true);
        assert_eq!(groups.len(), 2, "locked level emits left-edge row");
        let g1 = &groups[1];
        assert_eq!(g1.group_id, 1);
        assert!(matches!(g1.anchor, DecoratorAnchor::LeftEdge));
        assert_eq!(g1.items.len(), 1, "lock-only row has 1 item");
        match &g1.items[0].content {
            ItemContent::Badge(b) => {
                assert_eq!(b.segments[0].text, "LOCK");
            }
            _ => panic!("expected lock badge"),
        }
    }

    #[test]
    fn horizontal_level_to_decorators_packs_label_and_lock_in_one_group() {
        // A locked level with a label must emit a single left-anchored
        // decorator group whose items hold both the lock badge and the
        // label badge. Two separate groups would let their rects overlap
        // at the same LeftEdge anchor (BUG 1).
        let mut level = make_level(1, 100.0);
        level.label = Some("Support".into());
        let groups = level.to_decorators(true);
        assert_eq!(groups.len(), 2, "right price badge + packed left row");

        let left_groups: Vec<_> = groups
            .iter()
            .filter(|g| matches!(g.anchor, DecoratorAnchor::LeftEdge))
            .collect();
        assert_eq!(
            left_groups.len(),
            1,
            "label + lock must share one LeftEdge group"
        );
        let packed = left_groups[0];
        assert_eq!(packed.group_id, 1);
        assert_eq!(packed.items.len(), 2, "lock + label packed side-by-side");

        let texts: Vec<&str> = packed
            .items
            .iter()
            .filter_map(|it| match &it.content {
                ItemContent::Badge(b) => Some(b.segments[0].text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2);
        assert!(texts.contains(&"LOCK"), "lock item present");
        assert!(
            texts.iter().any(|t| t.contains("Support")),
            "label item present"
        );
        // Reading order: lock first, then label.
        assert_eq!(texts[0], "LOCK");
        assert!(texts[1].contains("Support"));
    }

    #[test]
    fn compute_level_label_and_lock_produce_non_overlapping_hit_rects() {
        // Regression for BUG 1: with both a label and a lock badge the
        // packed left-edge row must place the two badge rects
        // side-by-side on the x axis so neither the draw order nor
        // click dispatch is ambiguous.
        use crate::camera::Camera2D;
        use crate::widget::compute::{ComputeContext, Viewport};
        use crate::widget::level::compute_level;
        use crate::widget::theme::Theme;
        use crate::widget::AnnotationId;

        let camera = Camera2D {
            viewport_width: 400,
            viewport_height: 300,
            time_start: 0.0,
            time_end: 100_000.0,
            price_low: 100.0,
            price_high: 200.0,
            dpi_scale: 1.0,
        };
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

        let mut level = make_level(7, 150.0);
        level.label = Some("Resistance".into());
        let out = compute_level(&level, AnnotationId(7), &ctx, 1.0, true);

        // Both badges are now `BadgeShape::Rounded { radius: 0.0 }`
        // (still visually flat, but routes through the SDF pipeline so
        // the fill draws above the level line). Collect every
        // `BadgeInstance` whose rect sits on the left half of the
        // viewport — those are the lock + label badges (the price
        // badge lives on the right edge).
        let half_w = camera.viewport_width as f32 * 0.5;
        let left_rects: Vec<[f32; 4]> = out
            .badges
            .iter()
            .map(|b| b.rect)
            .filter(|r| r[0] < half_w)
            .collect();
        assert_eq!(
            left_rects.len(),
            2,
            "expected lock + label rects on left edge, got {left_rects:?}"
        );
        let a = left_rects[0];
        let b = left_rects[1];
        let (left, right) = if a[0] <= b[0] { (a, b) } else { (b, a) };
        assert!(
            left[2] <= right[0] + f32::EPSILON,
            "lock and label rects overlap on x axis: {left:?} vs {right:?}"
        );
    }
}
