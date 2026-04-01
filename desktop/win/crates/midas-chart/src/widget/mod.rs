//! Widget system: unified annotation architecture for chart overlays.
//!
//! All visual chart elements (levels, order brackets, indicators, notes,
//! markers) share a common compute→scene→GPU pipeline. This module defines
//! the core type system and per-variant data models.

pub mod bracket_tool;
pub mod compute;
pub mod hit_test;
pub mod level;
pub mod marker;
pub mod order_bracket;
pub mod text_note;
pub mod theme;

// ── Re-exports ─────────────────────────────────────────────────────

pub use self::bracket_tool::{BracketTool, BracketToolMode, BracketToolResult};
pub use self::compute::{ComputeContext, LabelAnchor, Viewport, WidgetLabel, WidgetOutput};
pub use self::hit_test::{BoundingBox, CursorIcon, HitResult, HitZone, HitZoneKind, Point};
pub use self::level::{HorizontalLevel, LevelExtend, LineStyle};
pub use self::marker::{MarkerAnnotation, MarkerIcon};
pub use self::order_bracket::{
    BracketLeg, BracketSide, BracketStatus, OrderBracket,
};
pub use self::text_note::TextNote;
pub use self::theme::Theme;

use midas_core::Timeframe;
use serde::{Deserialize, Serialize};
use std::fmt;

// ── AnnotationId ───────────────────────────────────────────────────

/// Monotonically increasing identifier for annotations.
///
/// Scoped to the `AnnotationStore` that owns it. Within a store, IDs
/// are never reused. IDs start at 1 so that `AnnotationId(0)` can
/// serve as a sentinel value.
///
/// Size: 8 bytes. Cheap to copy, hash, and compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnnotationId(pub u64);

impl AnnotationId {
    /// The null/sentinel value. No valid annotation has this ID.
    pub const NONE: Self = Self(0);

    /// Whether this is a valid (non-sentinel) ID.
    pub fn is_valid(self) -> bool {
        self.0 != 0
    }
}

impl fmt::Display for AnnotationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ann#{}", self.0)
    }
}

// ── Presence ───────────────────────────────────────────────────────

/// Three-tier visibility state for annotations.
///
/// Adapted from Bevy's visibility system. Determines whether an
/// annotation is rendered, interactive, or completely dormant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Presence {
    /// Fully rendered, interactive, hit-testable.
    #[default]
    Active,
    /// Rendered at reduced opacity. NOT interactive or hit-testable.
    Ghost,
    /// Not rendered at all. Zero GPU cost. Still in storage.
    Hidden,
}

impl Presence {
    /// Alpha multiplier for this presence state.
    pub fn alpha(&self) -> f32 {
        match self {
            Presence::Active => 1.0,
            Presence::Ghost => 0.4,
            Presence::Hidden => 0.0,
        }
    }

    /// Whether this annotation should respond to mouse events.
    pub fn is_interactive(&self) -> bool {
        matches!(self, Presence::Active)
    }

    /// Whether this annotation should be rendered.
    pub fn is_visible(&self) -> bool {
        !matches!(self, Presence::Hidden)
    }

    /// Whether this annotation should be included in hit-testing.
    pub fn is_hit_testable(&self) -> bool {
        self.is_interactive()
    }

    /// Transition to the next visibility state in the cycle:
    /// Active -> Ghost -> Hidden -> Active.
    pub fn cycle(&self) -> Self {
        match self {
            Presence::Active => Presence::Ghost,
            Presence::Ghost => Presence::Hidden,
            Presence::Hidden => Presence::Active,
        }
    }
}

// ── AnnotationKind ─────────────────────────────────────────────────

/// The specific widget type of an annotation.
///
/// This is a closed enum -- all variants are known at compile time.
/// Adding a new variant requires modifying this enum and every `match`
/// arm that dispatches on it. The compiler enforces exhaustiveness.
///
/// Only the `Level` variant is implemented in Phase 1A. Additional
/// variants (`OrderBracket`, `TextNote`, `Marker`) will be added in
/// their respective implementation phases.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// Horizontal line at a price. The most common annotation type.
    Level(HorizontalLevel),
    /// Entry + optional TP/SL bracket for order visualization.
    OrderBracket(OrderBracket),
    /// Text note anchored to a price/time point.
    TextNote(TextNote),
    /// Icon or stamp at a specific price/time.
    Marker(MarkerAnnotation),
}

// ── Annotation ─────────────────────────────────────────────────────

/// A chart annotation with metadata.
///
/// Every drawable element on a chart is an `Annotation`. The `kind`
/// field determines the specific widget type; the wrapper provides
/// shared metadata (ID, presence, timestamps, lock state).
///
/// Annotations are owned by `AnnotationStore` (in the app layer).
/// Charts receive `&[Annotation]` slices through `ChartInput`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    /// Unique identifier within the owning store.
    pub id: AnnotationId,
    /// The specific widget type and its data.
    pub kind: AnnotationKind,
    /// Visibility and interactivity state.
    pub presence: Presence,
    /// Optional timeframe filter. If `Some`, only rendered on charts
    /// with a matching timeframe. If `None`, rendered on all timeframes.
    pub visible_timeframes: Option<Vec<Timeframe>>,
    /// Whether this annotation is locked against drag/delete.
    pub locked: bool,
    /// Creation timestamp (epoch milliseconds). Set once, never changes.
    pub created_at: i64,
    /// Last modification timestamp (epoch milliseconds).
    pub modified_at: i64,
}

impl Annotation {
    /// Whether this annotation should be rendered on a chart with
    /// the given timeframe.
    pub fn should_render_on(&self, tf: Timeframe) -> bool {
        if !self.presence.is_visible() {
            return false;
        }
        match &self.visible_timeframes {
            None => true,
            Some(tfs) => tfs.contains(&tf),
        }
    }

    /// Whether this annotation should be included in hit-testing
    /// on a chart with the given timeframe.
    pub fn is_interactive_on(&self, tf: Timeframe) -> bool {
        self.should_render_on(tf) && self.presence.is_interactive()
    }

    /// Whether this annotation can be dragged (moved).
    pub fn is_draggable_on(&self, tf: Timeframe) -> bool {
        self.is_interactive_on(tf) && !self.locked
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels::LevelIcon;

    fn make_annotation(id: u64, price: f64) -> Annotation {
        Annotation {
            id: AnnotationId(id),
            kind: AnnotationKind::Level(HorizontalLevel {
                price,
                color: [0.0, 0.7, 1.0, 0.9],
                line_width: 1.0,
                style: LineStyle::default(),
                label: None,
                extend: LevelExtend::default(),
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
                assert!((level.price - 175.50).abs() < f64::EPSILON);
                assert_eq!(level.line_width, 1.0);
            }
            _ => panic!("expected Level variant"),
        }
    }

    #[test]
    fn horizontal_level_serde_round_trip() {
        let level = HorizontalLevel {
            price: 200.0,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 2.0,
            style: LineStyle::Dashed {
                dash_len: 8.0,
                gap_len: 4.0,
            },
            label: Some("Resistance".into()),
            extend: LevelExtend::FullWidth,
            icon: LevelIcon::Star,
        };
        let json = serde_json::to_string(&level).expect("serialize");
        let decoded: HorizontalLevel = serde_json::from_str(&json).expect("deserialize");
        assert!((decoded.price - 200.0).abs() < f64::EPSILON);
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
}
