//! Multi-segment badge: one outlined shape with one or more colored
//! segments laid out left-to-right inside.
//!
//! A segment may override the parent `shape`/`fill` (used for the "black
//! circle around 2" in the TP tag). `divider_color` draws a thin vertical
//! rule between adjacent segments. Hit testing walks segments linearly; a
//! segment carrying its own `DecoratorAction` emits a hit zone covering just
//! its sub-rect.

use super::action::DecoratorAction;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// One outlined shape containing one or more colored segments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Badge {
    /// Outer shape of the whole badge.
    pub shape: BadgeShape,
    /// Default fill color for segments that don't override it.
    pub fill: [f32; 4],
    /// Optional outline.
    pub border: Option<BadgeBorder>,
    /// Badge height in logical pixels.
    pub height: f32,
    /// Inner padding in logical pixels (applied to all four sides).
    pub padding: f32,
    /// One or more segments laid out in main-axis order.
    pub segments: SmallVec<[BadgeSegment; 3]>,
    /// When set, a thin vertical divider of this color is drawn between
    /// adjacent segments.
    pub divider_color: Option<[f32; 4]>,
}

/// A single colored compartment inside a `Badge`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BadgeSegment {
    /// Text rendered inside the segment. Iced paints this in an overlay
    /// pass on top of the GPU-rendered shape.
    pub text: String,
    /// RGBA text color.
    pub text_color: [f32; 4],
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Minimum segment width in logical pixels. Used to align columns
    /// across stacked legs (e.g., quantity segments on entry/TP/SL).
    pub min_width: Option<f32>,
    /// Per-segment fill override. `None` inherits the parent `Badge.fill`.
    pub fill_override: Option<[f32; 4]>,
    /// Per-segment shape override. `None` inherits the parent `Badge.shape`.
    pub shape_override: Option<BadgeShape>,
    /// When set, a click on this segment emits this action.
    pub action: Option<DecoratorAction>,
}

/// The geometric primitive painted for a `Badge` or overriding segment.
///
/// The variants are kept in discriminant order matching
/// `BadgeInstance::shape_id` in `crate::instances` and the `badge.wgsl`
/// fragment-shader switch. Reordering without updating the shader mapping
/// will corrupt rendering — enforced by
/// `badge_instance_shape_id_matches_enum` in `crate::instances` tests.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BadgeShape {
    /// Axis-aligned rectangle.
    Rect,
    /// Rounded rectangle with explicit corner radius.
    Rounded {
        /// Corner radius in logical pixels.
        radius: f32,
    },
    /// Pill (capsule): rectangle with fully-rounded short-axis ends.
    Pill,
    /// Rectangle with a left-pointing triangular nose of the given width.
    PointLeft {
        /// Horizontal width of the triangular nose in logical pixels.
        point_width: f32,
    },
    /// Rectangle with a right-pointing triangular nose of the given width.
    PointRight {
        /// Horizontal width of the triangular nose in logical pixels.
        point_width: f32,
    },
    /// Rectangle with triangular noses on both ends.
    DoublePoint {
        /// Horizontal width of each triangular nose in logical pixels.
        point_width: f32,
    },
    /// Asymmetric chevron (rightward pointing rhombus body).
    Chevron {
        /// Horizontal width of the chevron point in logical pixels.
        point_width: f32,
    },
    /// Circle. Diameter is `min(width, height)`.
    Circle,
}

impl BadgeShape {
    /// Stable discriminant for the GPU `BadgeInstance.shape_id` field.
    pub const fn shape_id(&self) -> u32 {
        match self {
            Self::Rect => 0,
            Self::Rounded { .. } => 1,
            Self::Pill => 2,
            Self::PointLeft { .. } => 3,
            Self::PointRight { .. } => 4,
            Self::DoublePoint { .. } => 5,
            Self::Chevron { .. } => 6,
            Self::Circle => 7,
        }
    }

    /// Shape parameter packed into the GPU `BadgeInstance.shape_param`.
    /// Variants without a parameter return `0.0`.
    pub const fn shape_param(&self) -> f32 {
        match self {
            Self::Rect | Self::Pill | Self::Circle => 0.0,
            Self::Rounded { radius } => *radius,
            Self::PointLeft { point_width }
            | Self::PointRight { point_width }
            | Self::DoublePoint { point_width }
            | Self::Chevron { point_width } => *point_width,
        }
    }
}

/// Optional outline painted around the badge body.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BadgeBorder {
    /// RGBA border color.
    pub color: [f32; 4],
    /// Border thickness in logical pixels.
    pub thickness: f32,
}
