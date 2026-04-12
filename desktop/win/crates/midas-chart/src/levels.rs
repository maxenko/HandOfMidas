//! Horizontal price levels.
//!
//! A `HorizontalLevel` represents a user-drawn horizontal line at a specific
//! price. Slice 7 of the decorator-system plan rewrote this type on top of
//! `PriceLine` + decorators, deleting the older `widget::level::HorizontalLevel`.
//! The old flat `color`/`line_width`/`style` fields now live inside
//! `line: PriceLine` and `to_decorators()` builds the standard level decorator
//! set (right-edge price badge, optional left-edge label/icon badge, optional
//! lock badge).
//!
//! ## Lock semantics
//!
//! Per the plan, `locked` conceptually lives on the `Annotation` wrapper
//! at `widget/mod.rs` rather than on the inner `HorizontalLevel`. That is
//! true for every level that flows through the `AnnotationStore`. The app's
//! pre-decorator `LevelStore` path still stores bare levels outside of an
//! `Annotation` wrapper (it predates the unified widget system), so a
//! sibling `StoredLevel { level, locked }` wrapper type lives in
//! `midas-app/src/level_store/mod.rs` and owns the lock flag for that path.
//! `HorizontalLevel::to_decorators(locked)` takes `locked` as an explicit
//! argument so both paths can drive the same decorator emission from a
//! wrapper-side flag.

use crate::widget::decorator::{
    Badge, BadgeSegment, BadgeShape, DecoratorAnchor, DecoratorGroup, DecoratorItem, FlexDirection,
    ItemContent, Visibility,
};
use crate::widget::price_line::{LineExtent, LineStroke, PriceLine};
use crate::widget::LineStyle;
use serde::de::{Deserializer, Error as DeError};
use serde::{Deserialize, Serialize};
use smallvec::smallvec;

/// Icon displayed next to a level label on the chart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LevelIcon {
    /// No icon.
    #[default]
    None,
    /// Upward arrow (bullish signal).
    ArrowUp,
    /// Downward arrow (bearish signal).
    ArrowDown,
    /// Star marker.
    Star,
    /// Flag marker.
    Flag,
    /// Warning/alert marker.
    Warning,
}

impl LevelIcon {
    /// Unicode character representation for rendering on the chart.
    pub fn as_char(&self) -> Option<char> {
        match self {
            LevelIcon::None => None,
            LevelIcon::ArrowUp => Some('\u{25B2}'),   // ▲
            LevelIcon::ArrowDown => Some('\u{25BC}'), // ▼
            LevelIcon::Star => Some('\u{2726}'),      // ✦
            LevelIcon::Flag => Some('\u{2691}'),      // ⚑
            LevelIcon::Warning => Some('\u{26A0}'),   // ⚠
        }
    }

    /// Display name for the icon (used in UI selectors).
    pub fn display_name(&self) -> &'static str {
        match self {
            LevelIcon::None => "None",
            LevelIcon::ArrowUp => "Arrow Up",
            LevelIcon::ArrowDown => "Arrow Down",
            LevelIcon::Star => "Star",
            LevelIcon::Flag => "Flag",
            LevelIcon::Warning => "Warning",
        }
    }

    /// All available icon variants for UI selection.
    pub fn all() -> &'static [LevelIcon] {
        &[
            LevelIcon::None,
            LevelIcon::ArrowUp,
            LevelIcon::ArrowDown,
            LevelIcon::Star,
            LevelIcon::Flag,
            LevelIcon::Warning,
        ]
    }

    /// Convert from a string identifier (used in config persistence).
    pub fn from_str_id(s: &str) -> Self {
        match s {
            "arrow_up" => LevelIcon::ArrowUp,
            "arrow_down" => LevelIcon::ArrowDown,
            "star" => LevelIcon::Star,
            "flag" => LevelIcon::Flag,
            "warning" => LevelIcon::Warning,
            _ => LevelIcon::None,
        }
    }

    /// Convert to a string identifier (used in config persistence).
    pub fn to_str_id(&self) -> &'static str {
        match self {
            LevelIcon::None => "none",
            LevelIcon::ArrowUp => "arrow_up",
            LevelIcon::ArrowDown => "arrow_down",
            LevelIcon::Star => "star",
            LevelIcon::Flag => "flag",
            LevelIcon::Warning => "warning",
        }
    }
}

/// Compute a smart price step size based on the current price level.
///
/// Returns `(coarse_step, fine_step)` where coarse is for arrow key clicks
/// and fine is for Shift+arrow or scroll wheel.
pub fn price_step_for(price: f64) -> (f64, f64) {
    if price.abs() >= 200.0 {
        (0.05, 0.05)
    } else {
        (0.01, 0.01)
    }
}

/// A user-defined horizontal price level.
///
/// Represents a horizontal line drawn at a specific price on the chart.
/// Wraps a `PriceLine` geometry primitive plus the level's domain metadata
/// (`label`, `icon`). The `locked` flag is carried on the `Annotation`
/// wrapper for the widget path and on a `StoredLevel` wrapper for the
/// pre-decorator `LevelStore` path — see the module doc.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HorizontalLevel {
    /// Unique identifier for this level within the chart.
    pub id: u64,
    /// Line geometry (price, extent, stroke: color/width/style).
    pub line: PriceLine,
    /// Optional text label displayed on the chart next to the line.
    pub label: Option<String>,
    /// Optional icon displayed next to the label.
    pub icon: LevelIcon,
}

// ── Two-version deserialize strategy ───────────────────────────────
//
// `#[serde(untagged)]` usually picks the first variant that deserializes
// successfully, which is a silent-branch-selection risk when v1 and v2
// overlap on fields like `id`/`label`/`icon`. We mitigate the ambiguity
// by marking V2 with `deny_unknown_fields` AND requiring the nested
// `line` field — a V1 payload has `price`/`color`/`line_width` at the
// top level, which V2 rejects as unknown fields, deterministically
// falling through to V1. V2 always has the `line` key, so V1 rejects
// it via its own flat schema. The discriminator is therefore the
// presence of `line`, and untagged selection becomes unambiguous. This
// approach is cross-format (works for both JSON via `serde_json` and
// TOML via `toml`) without needing to buffer through an intermediate
// `Value` type, and avoids pulling in `serde-value` as a new dep.
//
// If either branch grows new overlapping fields in the future, the
// `deny_unknown_fields` + mandatory `line` pair must be preserved or
// the discriminator breaks.

#[derive(Deserialize)]
#[serde(untagged)]
enum HorizontalLevelRepr {
    V2(HorizontalLevelV2),
    V1(HorizontalLevelV1),
}

/// V2 (post-Slice-7) shape used to drive `Deserialize`. Private.
/// `deny_unknown_fields` + the mandatory nested `line` field is the
/// discriminator against the v1 shape.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HorizontalLevelV2 {
    id: u64,
    line: PriceLine,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    icon: LevelIcon,
}

/// V1 (pre-Slice-7) flat shape. Fallback branch. The `locked` field is
/// accepted for migration compatibility but dropped from the in-memory
/// type — a one-time warn is logged if it was `true`, since the caller
/// needs to set `Annotation.locked` or `StoredLevel.locked` on the
/// wrapper manually.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HorizontalLevelV1 {
    id: u64,
    price: f64,
    color: [f32; 4],
    #[serde(default = "default_line_width_v1")]
    line_width: f32,
    #[serde(default)]
    style: LineStyle,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    icon: LevelIcon,
    // Accept-and-drop: v1 `extend` (LevelExtend) is no longer a
    // distinct type. We accept whatever the old JSON/TOML encoded and
    // fold the result into `LineExtent::FullWidth`. Callers that need
    // a non-default extent must re-save the level.
    #[serde(default, deserialize_with = "deserialize_ignored_extend")]
    #[allow(dead_code)]
    extend: (),
    #[serde(default)]
    locked: bool,
}

fn default_line_width_v1() -> f32 {
    1.0
}

/// Accept any shape for the legacy `extend` field and discard it.
/// Used by `HorizontalLevelV1` so v1 payloads with `"extend":"FullWidth"`
/// or `"extend":{"RightFrom":{"timestamp":123}}` both parse.
fn deserialize_ignored_extend<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(())
}

impl From<HorizontalLevelV2> for HorizontalLevel {
    fn from(v: HorizontalLevelV2) -> Self {
        Self {
            id: v.id,
            line: v.line,
            label: v.label,
            icon: v.icon,
        }
    }
}

impl From<HorizontalLevelV1> for HorizontalLevel {
    fn from(v: HorizontalLevelV1) -> Self {
        if v.locked {
            tracing::warn!(
                level_id = v.id,
                "v1 HorizontalLevel had locked=true; drop this field — set \
                 Annotation.locked or StoredLevel.locked on the wrapper instead"
            );
        }
        Self {
            id: v.id,
            line: PriceLine {
                price: v.price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: v.color,
                    width: v.line_width,
                    style: v.style,
                },
            },
            label: v.label,
            icon: v.icon,
        }
    }
}

impl<'de> Deserialize<'de> for HorizontalLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = HorizontalLevelRepr::deserialize(deserializer).map_err(|e| {
            DeError::custom(format!(
                "HorizontalLevel: neither v2 nor v1 shape matched: {e}"
            ))
        })?;
        match repr {
            HorizontalLevelRepr::V2(v2) => Ok(v2.into()),
            HorizontalLevelRepr::V1(v1) => Ok(v1.into()),
        }
    }
}

impl HorizontalLevel {
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
    /// `locked` is sourced from the wrapper (`Annotation.locked` or the
    /// level-store-side `StoredLevel.locked`), not from the level itself.
    pub fn to_decorators(&self, locked: bool) -> Vec<DecoratorGroup> {
        let mut groups: Vec<DecoratorGroup> = Vec::new();
        let line_color = self.line.stroke.color;

        // Group 0: right-edge price badge.
        // `action: None` in Slice 7 — clicks fall through to the
        // existing `HitZoneKind::LevelLine` drag hit zone emitted by
        // `compute_price_line_geometry`.
        groups.push(DecoratorGroup {
            group_id: 0,
            anchor: DecoratorAnchor::RightEdge,
            direction: FlexDirection::Row,
            gap: 0.0,
            items: smallvec![DecoratorItem {
                visibility: Visibility::Always,
                action: None,
                content: ItemContent::Badge(Box::new(Badge {
                    shape: BadgeShape::Rect,
                    fill: [0.12, 0.12, 0.15, 0.85],
                    border: None,
                    height: 18.0,
                    padding: 6.0,
                    segments: smallvec![BadgeSegment {
                        text: format!("{:.2}", self.line.price),
                        text_color: line_color,
                        font_size: 11.0,
                        min_width: None,
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

            if locked {
                items.push(DecoratorItem {
                    visibility: Visibility::Always,
                    action: None,
                    content: ItemContent::Badge(Box::new(Badge {
                        shape: BadgeShape::Rect,
                        fill: [0.25, 0.25, 0.30, 0.85],
                        border: None,
                        height: 18.0,
                        padding: 6.0,
                        segments: smallvec![BadgeSegment {
                            // U+1F512 lock glyph may not ship with the
                            // standard font — use the plain text "LOCK"
                            // tag which every font renders.
                            text: "LOCK".to_owned(),
                            text_color: [1.0, 1.0, 1.0, 1.0],
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
                let fill = [
                    line_color[0] * 0.3,
                    line_color[1] * 0.3,
                    line_color[2] * 0.3,
                    0.75,
                ];
                let text_color = [
                    line_color[0],
                    line_color[1],
                    line_color[2],
                    line_color[3].max(0.9),
                ];
                items.push(DecoratorItem {
                    visibility: Visibility::Always,
                    action: None,
                    content: ItemContent::Badge(Box::new(Badge {
                        shape: BadgeShape::Rect,
                        fill,
                        border: None,
                        height: 20.0,
                        padding: 6.0,
                        segments: smallvec![BadgeSegment {
                            text: icon_text,
                            text_color,
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
        assert!(matches!(g0.anchor, DecoratorAnchor::RightEdge));
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
        assert_eq!(
            groups.len(),
            1,
            "no label, icon, or lock: only group 0"
        );
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
        assert!(texts.iter().any(|t| *t == "LOCK"), "lock item present");
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

        // Both badges use `BadgeShape::Rect`, which routes through
        // `WidgetOutput.fills`. Collect every fill rect that sits on
        // the left half of the viewport — those are the lock + label
        // badges (the price badge lives on the right edge).
        let half_w = camera.viewport_width as f32 * 0.5;
        let left_rects: Vec<[f32; 4]> = out
            .fills
            .iter()
            .map(|g| g.rect)
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
