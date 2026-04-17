//! Decorator group: a flex-laid container of badges, buttons, nested groups,
//! and spacers anchored to a point on a parent `PriceLine`.

use super::action::DecoratorAction;
use super::badge::Badge;
use super::button::Button;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// One flex container anchored to a point on a parent `PriceLine`.
///
/// `group_id` is stable within the parent annotation's set of groups — it is
/// **not** globally unique. The hover-persistence layer uses
/// `(AnnotationId, group_id)` as the composite expansion key, and the
/// click-routing layer uses the same pair to disambiguate clicks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecoratorGroup {
    /// Stable identifier unique within the parent annotation.
    ///
    /// Must be unique within one annotation's decorator set; collisions
    /// cause undefined click routing.
    pub group_id: u16,
    /// Anchor point on the parent `PriceLine`.
    pub anchor: DecoratorAnchor,
    /// Main-axis direction for the flex layout.
    pub direction: FlexDirection,
    /// Gap in logical pixels between adjacent items along the main axis.
    pub gap: f32,
    /// Items in main-axis order.
    pub items: SmallVec<[DecoratorItem; 4]>,
}

/// Where on the parent `PriceLine` a `DecoratorGroup` pins itself.
///
/// Anchors only control the X axis; the Y component always comes from
/// `camera.price_to_y(parent_line.price)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum DecoratorAnchor {
    /// Pinned to the left edge of the viewport.
    LeftEdge,
    /// Pinned to the right edge of the viewport.
    RightEdge,
    /// Pinned to the chart-area right edge (viewport right minus the
    /// price-axis area), offset further left by `pointer_inset`. Items
    /// pack forward (left-to-right) from this anchor, unlike
    /// `RightEdge` which packs right-to-left.
    ///
    /// `pointer_inset` compensates for shapes that have a protruding
    /// side (e.g. `BadgeShape::PointLeft` extends `point_width` pixels
    /// left of the badge body). Set it to the shape's point width and
    /// the body's left edge — the triangle base — lands exactly on the
    /// vertical priceline border with the tip sticking into the chart.
    /// Use `0.0` for shapes without a left-side pointer.
    AtChartRightEdge { pointer_inset: f32 },
    /// Pinned to a chart-time (epoch ms) coordinate.
    AtTimestamp(i64),
    /// Pinned to a raw screen-X coordinate in logical pixels.
    AtScreenX(f32),
}

/// Main-axis direction for flex layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum FlexDirection {
    /// Horizontal layout: children stack left-to-right (or right-to-left
    /// when the anchor is right-aligned — see `05-interaction.md`).
    Row,
    /// Vertical layout: children stack top-to-bottom.
    Column,
}

/// One item inside a `DecoratorGroup`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecoratorItem {
    /// When this item should be emitted during a frame.
    pub visibility: Visibility,
    /// Optional click action. When set, clicks within this item's rect emit
    /// a `ChartAction::DecoratorClick` carrying this action.
    pub action: Option<DecoratorAction>,
    /// What this item contains.
    pub content: ItemContent,
}

/// When a `DecoratorItem` should be emitted during a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    /// Always emitted. Used for permanent badges (price, label, quantity).
    #[default]
    Always,
    /// Emitted only while the parent `PriceLine` is hovered. Cosmetic
    /// affordances that should not persist past the cursor moving off.
    OnLineHover,
    /// Emitted while the parent line is hovered **OR** while any currently-
    /// visible item in the same group is hovered. Used for click targets
    /// that must stay alive long enough to be clicked after the cursor
    /// leaves the line.
    OnGroupHover,
}

/// What a `DecoratorItem` contains. `Badge` and `Stack` are boxed to keep
/// `ItemContent` small; a nested `Stack` group's `anchor` is ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ItemContent {
    /// A multi-segment badge.
    Badge(Box<Badge>),
    /// A single-shape button.
    Button(Button),
    /// A nested group (e.g. the `▲`/`▼` column inside a row group).
    Stack(Box<DecoratorGroup>),
    /// A fixed-width gap along the main axis.
    Spacer(f32),
}
