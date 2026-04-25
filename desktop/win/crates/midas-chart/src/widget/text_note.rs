//! Re-export shim for `TextNote`.
//!
//! Moved to `midas-annotation-types::text_note` in Slice A1 because it
//! is a pure-data variant of the moved `AnnotationKind`. This file is
//! preserved as a shim so existing
//! `midas_chart::widget::text_note::*` import paths keep resolving.
//! A1b added `#[deprecated]` after consumer-side migration is done.

#[deprecated(
    note = "import from midas_annotation_types directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_annotation_types::text_note::TextNote;
