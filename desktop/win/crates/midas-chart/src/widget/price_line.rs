//! Re-export shim for `PriceLine`, `LineStroke`, `LineExtent`.
//!
//! The data types moved to `midas-annotation-types` in Slice A1. This
//! file is preserved as a shim so existing
//! `midas_chart::widget::price_line::*` import paths keep resolving.
//! A1b added `#[deprecated]` after consumer-side migration so new
//! imports through this path are caught by clippy as errors.

#[deprecated(
    note = "import from midas_annotation_types directly; midas-chart will be deleted in slice 9c"
)]
pub use midas_annotation_types::{LineExtent, LineStroke, PriceLine};
