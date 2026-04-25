//! Re-export shim for `MarkerAnnotation` and `MarkerIcon`.
//!
//! Both moved to `midas-annotation-types::marker` in Slice A1 because
//! they are pure-data variants of the moved `AnnotationKind`. This
//! file is preserved as a shim so existing
//! `midas_chart::widget::marker::*` import paths keep resolving. A1b
//! added `#[deprecated]` after consumer-side migration is done.

#[deprecated(
    note = "import from midas_annotation_types directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_annotation_types::marker::{MarkerAnnotation, MarkerIcon};
