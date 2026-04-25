//! Widget system: unified annotation architecture for chart overlays.
//!
//! All visual chart elements (levels, order brackets, indicators, notes,
//! markers) share a common compute→scene→GPU pipeline. This module defines
//! the core type system and per-variant data models.

pub mod bracket_tool;
pub mod compute;
pub mod decorator;
pub mod hit_test;
pub mod level;
pub mod marker;
pub mod order_bracket;
pub mod price_line;
pub mod text_note;
pub mod theme;

// ── Re-exports ─────────────────────────────────────────────────────

pub use self::bracket_tool::{BracketTool, BracketToolMode, BracketToolResult};
pub use self::compute::{ComputeContext, LabelAnchor, Viewport, WidgetLabel, WidgetOutput};
pub use self::decorator::{
    Badge, BadgeBorder, BadgeSegment, BadgeShape, Button, DecoratorAction, DecoratorAnchor,
    DecoratorGroup, DecoratorItem, FlexDirection, ItemContent, Visibility,
};
pub use self::hit_test::{
    BoundingBox, CursorIcon, HitResult, HitZone, HitZoneKind, ItemPath, Point,
};
// `LineStyle`, `PriceLine`, `LineStroke`, `LineExtent`, `HorizontalLevel`,
// `MarkerAnnotation`/`MarkerIcon`, `TextNote`, the order-bracket data tree,
// and the `Annotation`/`AnnotationId`/`AnnotationKind`/`Presence` envelope
// all moved to `midas-annotation-types` in Slice A1. The chart-only
// `compute_*`/`segmented_line` helpers stayed behind in
// `crate::widget::level` and `crate::widget::order_bracket`.
pub use self::level::{compute_level, compute_price_line_geometry, segmented_line, LineStyle};
pub use self::marker::{MarkerAnnotation, MarkerIcon};
pub use self::order_bracket::{BracketLeg, BracketSide, BracketStatus, EntryType, OrderBracket};
pub use self::price_line::{LineExtent, LineStroke, PriceLine};
pub use self::text_note::TextNote;
pub use self::theme::Theme;
pub use crate::levels::HorizontalLevel;

// Annotation envelope (moved). Re-exported via `midas_chart::widget::*`
// for back-compat with the many downstream call sites that imported
// these names from this module.
//
// A1b added `#[deprecated]` after consumer-side migration so any new
// import through `midas_chart::widget::{Annotation, AnnotationId, ...}`
// is caught by clippy as a hard error under `-D warnings`.
#[deprecated(
    note = "import from midas_annotation_types directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_annotation_types::{Annotation, AnnotationId, AnnotationKind, Presence};

#[cfg(test)]
mod tests;
