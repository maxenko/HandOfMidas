//! Text note annotation type.
//!
//! A text note anchored to a price/time point on the chart. Rendered
//! as a colored rectangle background with text on top via the iced
//! overlay layer.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let note = TextNote {
            price: 185.0,
            timestamp: 1700000000000,
            text: "Support zone".into(),
            background_color: [0.15, 0.15, 0.2, 0.85],
            text_color: [0.9, 0.9, 0.9, 1.0],
            font_size: 12.0,
            max_width: Some(200.0),
        };
        let json = serde_json::to_string(&note).expect("serialize");
        let decoded: TextNote = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.text, "Support zone");
        assert!((decoded.price - 185.0).abs() < f64::EPSILON);
        assert_eq!(decoded.max_width, Some(200.0));
    }

    #[test]
    fn serde_without_max_width() {
        let note = TextNote {
            price: 100.0,
            timestamp: 0,
            text: "Test".into(),
            background_color: [0.0; 4],
            text_color: [1.0; 4],
            font_size: 11.0,
            max_width: None,
        };
        let json = serde_json::to_string(&note).expect("serialize");
        let decoded: TextNote = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.max_width, None);
    }
}
