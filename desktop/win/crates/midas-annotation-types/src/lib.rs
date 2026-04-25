//! Persistent annotation type tree, extracted from `midas-chart` so
//! the chart crate can be retired without dragging the wire-format
//! types with it (Slice A1 of `plan/arch-review-fixes/01-group-a-...`).
//!
//! This crate carries **data only**: the `Annotation` envelope, the
//! `AnnotationKind` enum and its variants, the `HorizontalLevel` /
//! `OrderBracket` / `TextNote` / `MarkerAnnotation` payload shapes,
//! and the `PriceLine` / `LineStroke` / `LineExtent` / `LineStyle`
//! geometry primitives shared across them. Compute / hit-test /
//! decorator-emission helpers stay in `midas-chart` — they depend on
//! GPU-instance and `ComputeContext` types that are out of scope here.
//!
//! ## Wire-format lock
//!
//! Every type re-exported here is serialized to disk by the app layer
//! (`midas-app/src/annotation_persistence.rs`). The variant tags on
//! `AnnotationKind` are pinned with explicit `#[serde(rename = "...")]`
//! attributes that match the existing on-disk PascalCase format
//! (`tests/fixtures/annotations_v1_pre_decorator.json`). The
//! `HorizontalLevel` `Deserialize` impl carries the V1/V2 forward-
//! compat path verbatim.

pub mod annotation;
pub mod levels;
pub mod line_style;
pub mod marker;
pub mod order_bracket;
pub mod price_line;
pub mod text_note;

// Re-exports for the canonical import paths. Mirrors what
// `midas-chart::widget` and `midas-chart::levels` used to expose.
pub use annotation::{Annotation, AnnotationId, AnnotationKind, Presence};
pub use levels::{price_step_for, HorizontalLevel, LevelIcon};
pub use line_style::LineStyle;
pub use marker::{MarkerAnnotation, MarkerIcon};
pub use order_bracket::{
    is_leg_on_wrong_side, BracketLeg, BracketSide, BracketStatus, EntryType, LegRole, OrderBracket,
    BRACKET_LONG_ENTRY_COLOR, BRACKET_LONG_STOP_COLOR, BRACKET_LONG_STOP_LIMIT_COLOR,
    BRACKET_SHORT_ENTRY_COLOR, BRACKET_SHORT_STOP_COLOR, BRACKET_SHORT_STOP_LIMIT_COLOR,
    BRACKET_SL_COLOR, BRACKET_TP_COLOR, BRACKET_WARNING_COLOR,
};
pub use price_line::{LineExtent, LineStroke, PriceLine};
pub use text_note::TextNote;
