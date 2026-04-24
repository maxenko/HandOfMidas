//! Decorator subsystem (slice 5a of the chart-transition plan).
//!
//! A decorator is a small piece of chart UI — a badge, a button, a line,
//! a spacer — that hangs off a parent annotation (a level, a bracket
//! leg, an indicator). Decorators compose into [`DecoratorGroup`]s with
//! per-group [`Visibility`] rules so the chart can reveal click-targets
//! only while the parent annotation is hovered, while keeping permanent
//! labels (price, side) always visible.
//!
//! ## Relationship to the legacy implementation
//!
//! The legacy desktop-workspace decorator subsystem
//! (`desktop/win/crates/midas-chart/src/widget/decorator/`) is the port
//! source. The sans-IO port here is intentionally narrower:
//!
//! | Feature | Legacy | Slice 5a |
//! | --- | --- | --- |
//! | Hierarchical groups | yes (`Stack`) | no (flat items) |
//! | Flex direction | `Row` / `Column` | caller supplies absolute rects |
//! | Anchor resolution | `DecoratorAnchor` enum | caller supplies absolute rects |
//! | Badge segment machinery | `BadgeSegment` with divider colour | one-shot `BadgeInstance` |
//! | Proximity promotion | 20 px radius (legacy) | 32 px radius (plan spec) |
//! | Drag ghost | `Presence::Ghost` chains alpha | layer-driven alpha = 0.5 |
//!
//! The plan documents this narrowing as R18 — a deliberate 54% test-
//! coverage cut (60 tests vs legacy ~130). Consumers that need richer
//! anchoring wrap this module from `midas-app` / `midas-render`; the
//! scene crate stays dep-light.
//!
//! ## Sub-z bands
//!
//! Per plan slice 5a, the layer uses four sub-z bands, drawn in
//! ascending order (painter's algorithm — later passes paint on top):
//!
//! 0. Background. Default for any item that is neither proximity-
//!    promoted, hovered, nor being dragged.
//! 1. Proximity-promoted. Cursor is within
//!    [`layout::PROXIMITY_THRESHOLD_PX`] of the group's parent bounds.
//! 2. Hovered. Cursor is directly over the item (button / badge rect).
//! 3. Dragged. Owning annotation is in a drag session — alpha = 0.5
//!    applied layer-wide, original still paints at sub_z 0.
//!
//! ## Public surface
//!
//! - [`DecoratorGroup`] — container: `items`, `bounds`, `visibility`.
//! - [`DecoratorItem`] — one of `Line`, `Badge`, `Button`, `Spacer`.
//! - [`Visibility`] — `Always` or `OnHover { parent }`.
//! - [`HoverState`] — per-frame cursor / hover / drag snapshot.
//! - [`Rect`] — simple axis-aligned rectangle.
//! - [`ButtonAction`] — what clicking a `Button` item emits.
//! - [`GroupId`] — opaque group identifier (u64 newtype).

use crate::primitives::{BadgeInstance, LineInstance};
use crate::tools::{AnnotationId, ContextMenuAction, ToolEffect};

pub mod layout;

pub use layout::{
    apply_drag_ghost_alpha, emissions_for_group, promote_by_proximity, visibility_for,
    DecoratorEmission, PromotedItem, SubZ, DRAG_GHOST_ALPHA, PROXIMITY_THRESHOLD_PX,
};

#[cfg(test)]
mod tests;

/// Opaque group identifier. Stable within one parent annotation but not
/// globally unique; the layer composes `(annotation, group)` to route
/// button clicks and hover-expand toggles.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GroupId(pub u64);

/// Axis-aligned rectangle in viewport pixel space. Top-left origin;
/// inclusive on all edges for hit-testing (matches legacy convention
/// `widget::decorator::compute::rect_contains`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    /// Construct from two opposing corners; normalises so `x0 <= x1`
    /// and `y0 <= y1`.
    #[inline]
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }

    /// Width (`x1 - x0`).
    #[inline]
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    /// Height (`y1 - y0`).
    #[inline]
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    /// Whether `(x, y)` lies inside (inclusive on every edge).
    #[inline]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Minimum vertical distance from `y` to the rectangle's y span.
    /// Zero when `y` is inside `[y0, y1]`.
    #[inline]
    pub fn vertical_distance(&self, y: f32) -> f32 {
        if y < self.y0 {
            self.y0 - y
        } else if y > self.y1 {
            y - self.y1
        } else {
            0.0
        }
    }

    /// Euclidean distance from `(x, y)` to the rectangle's closest edge.
    /// Zero when the point is inside.
    #[inline]
    pub fn distance_to(&self, x: f32, y: f32) -> f32 {
        let dx = if x < self.x0 {
            self.x0 - x
        } else if x > self.x1 {
            x - self.x1
        } else {
            0.0
        };
        let dy = if y < self.y0 {
            self.y0 - y
        } else if y > self.y1 {
            y - self.y1
        } else {
            0.0
        };
        (dx * dx + dy * dy).sqrt()
    }
}

/// When a [`DecoratorGroup`] should emit its items.
///
/// - `Always` — the group paints on every frame (permanent price
///   labels, side chips).
/// - `OnHover { parent }` — the group paints only while the cursor is
///   inside the bounds of the annotation named by `parent`, OR while
///   `parent` appears in [`HoverState::expanded_groups`] (sticky
///   expansion after clicking an "expand" button). `parent == None`
///   means the group's own bounds gate the reveal (stand-alone chips).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Visibility {
    #[default]
    Always,
    OnHover {
        parent: Option<AnnotationId>,
    },
}

/// Effect emitted when a `Button` decorator item is clicked.
///
/// Keeping the action narrow on purpose: the layer translates a hit on
/// a button into a [`ToolEffect`] which the scene drains through its
/// existing effect queue. The legacy `DecoratorAction` enum carried
/// richer variants (per-bracket-leg "expand" toggles etc.) — those
/// higher-level semantics live in the app translation layer, not the
/// scene crate.
#[derive(Clone, Debug, PartialEq)]
pub enum ButtonAction {
    /// Open a context menu at the click point.
    OpenContextMenu {
        items: Vec<crate::tools::ContextMenuItem>,
    },
    /// Direct, well-known action (Edit / Lock / Delete on an
    /// annotation). The layer translates into the corresponding
    /// [`ToolEffect`] variant at click time.
    Menu(ContextMenuAction),
    /// Any other effect — a future button that, say, toggles a layer
    /// toolbar chip can slot a pre-built `ToolEffect` here without
    /// expanding this enum.
    Effect(ToolEffect),
}

/// One item inside a [`DecoratorGroup`]. Every variant carries its own
/// absolute pixel geometry — the layer does not run its own flex-layout
/// pass, per the "caller supplies absolute rects" narrowing.
#[derive(Clone, Debug, PartialEq)]
pub enum DecoratorItem {
    /// A line segment (bracket leg, tick mark). Emits directly into
    /// `ScenePrimitives::lines`.
    Line(LineInstance),
    /// A filled, labelled badge. Emits directly into
    /// `ScenePrimitives::badges`.
    Badge(BadgeInstance),
    /// A clickable region. Emits a [`BadgeInstance`] so the GPU
    /// renderer has something to draw + a hit-test rect the layer uses
    /// to route clicks.
    Button {
        bounds: Rect,
        color: [u8; 4],
        label: std::borrow::Cow<'static, str>,
        action: ButtonAction,
    },
    /// A fixed-size empty gap. Contributes no primitives; carried so
    /// the layer preserves insertion order when the host pre-lays-out a
    /// group and wants to keep indices stable.
    Spacer { w: f32, h: f32 },
}

impl DecoratorItem {
    /// Best-effort bounding rect for proximity + hover hit-testing.
    /// Badges and buttons report their own rect; lines report a zero-
    /// width band around their endpoints; spacers have zero area.
    pub fn bounds(&self) -> Rect {
        match self {
            DecoratorItem::Line(l) => Rect::new(l.x0, l.y0, l.x1, l.y1),
            DecoratorItem::Badge(b) => Rect::new(b.x, b.y, b.x + b.w, b.y + b.h),
            DecoratorItem::Button { bounds, .. } => *bounds,
            DecoratorItem::Spacer { .. } => Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }

    /// `true` if the item emits a drawable primitive. Spacers return
    /// `false`; every other variant returns `true`.
    pub fn is_drawable(&self) -> bool {
        !matches!(self, DecoratorItem::Spacer { .. })
    }
}

/// A flat container of decorator items with a single visibility rule
/// and a parent-annotation bounding box used for hover gating.
#[derive(Clone, Debug, PartialEq)]
pub struct DecoratorGroup {
    /// Stable identifier — unique within the parent annotation.
    pub id: GroupId,
    /// The annotation that owns this group. Used to pick up hover,
    /// drag-ghost, and expansion state out of [`HoverState`].
    pub annotation: AnnotationId,
    /// Parent bounding box — the hover-reveal region for `OnHover`
    /// children. Defaults to an empty rect; the host supplies the
    /// annotation's geometry once per frame.
    pub parent_bounds: Rect,
    /// When this group should emit.
    pub visibility: Visibility,
    /// Items in insertion order. Order is preserved by the layer for
    /// same-sub_z tie-breaking.
    pub items: Vec<DecoratorItem>,
}

impl DecoratorGroup {
    /// Convenience constructor: an always-visible group.
    pub fn always(
        id: GroupId,
        annotation: AnnotationId,
        parent_bounds: Rect,
        items: Vec<DecoratorItem>,
    ) -> Self {
        Self {
            id,
            annotation,
            parent_bounds,
            visibility: Visibility::Always,
            items,
        }
    }

    /// Convenience constructor: a hover-only group anchored to the
    /// parent annotation's bounds (no independent parent-annotation
    /// chain).
    pub fn on_hover(
        id: GroupId,
        annotation: AnnotationId,
        parent_bounds: Rect,
        items: Vec<DecoratorItem>,
    ) -> Self {
        Self {
            id,
            annotation,
            parent_bounds,
            visibility: Visibility::OnHover { parent: None },
            items,
        }
    }
}

/// Per-frame hover / drag snapshot. The host widget rebuilds this at
/// the start of every paint cycle so the decorator layer stays pure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoverState {
    /// Annotation the cursor is currently hovering (if any).
    pub hovered_annotation: Option<AnnotationId>,
    /// Annotation being dragged (if any). Triggers sub_z = 3 + alpha
    /// blend for every group owned by this annotation.
    pub dragged_annotation: Option<AnnotationId>,
    /// Groups the user has "expanded" by clicking an expand button.
    /// Treated as sticky hover — their `OnHover` children stay visible
    /// even when the cursor moves off the parent bounds.
    pub expanded_groups: Vec<GroupId>,
    /// Cursor position in viewport pixels. `None` when the cursor is
    /// off the chart surface — in which case no proximity promotion
    /// fires.
    pub cursor_px: Option<crate::input::Point>,
}

impl HoverState {
    /// `true` iff `group` is expanded.
    pub fn is_group_expanded(&self, group: GroupId) -> bool {
        self.expanded_groups.contains(&group)
    }

    /// `true` iff `annotation` is the currently-hovered annotation.
    pub fn is_annotation_hovered(&self, annotation: AnnotationId) -> bool {
        self.hovered_annotation == Some(annotation)
    }

    /// `true` iff `annotation` is being dragged.
    pub fn is_annotation_dragged(&self, annotation: AnnotationId) -> bool {
        self.dragged_annotation == Some(annotation)
    }
}
