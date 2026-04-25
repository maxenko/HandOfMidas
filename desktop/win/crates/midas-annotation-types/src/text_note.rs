//! Text note annotation type.
//!
//! Moved from `midas-chart/src/widget/text_note.rs` (Slice A1) —
//! required here because `AnnotationKind::TextNote(TextNote)` is a
//! variant of the moved enum. Pure data with no chart-only deps;
//! rendering uses these fields via `midas-chart`'s overlay layer.

use serde::{Deserialize, Serialize};

/// A text note anchored to a price/time point on the chart.
///
/// Rendered as a colored rectangle background with text on top.
/// The background is a GPU `GridLineInstance`; the text is rendered
/// by the iced overlay layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextNote {
    /// Anchor price (Y position).
    pub price: f64,
    /// Anchor timestamp (X position), epoch milliseconds.
    pub timestamp: i64,
    /// The note text content.
    pub text: String,
    /// Background color for the note rectangle.
    pub background_color: [f32; 4],
    /// Text color.
    pub text_color: [f32; 4],
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Maximum width in logical pixels for word wrapping.
    /// None = single line, no wrapping.
    pub max_width: Option<f32>,
}
