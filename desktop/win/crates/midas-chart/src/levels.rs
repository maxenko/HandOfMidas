//! Horizontal price levels.
//!
//! A `HorizontalLevel` represents a user-drawn horizontal line at a specific
//! price. These are persisted with the chart state and serialized to config.

/// A user-defined horizontal price level.
///
/// Represents a horizontal line drawn at a specific price on the chart.
/// Supports serialization for persistence across sessions.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HorizontalLevel {
    /// Unique identifier for this level within the chart.
    pub id: u64,
    /// Price at which the horizontal line is drawn.
    pub price: f64,
    /// RGBA color of the line (linear space).
    pub color: [f32; 4],
    /// Line width in logical pixels.
    pub line_width: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_clone_and_debug() {
        let level = HorizontalLevel {
            id: 1,
            price: 150.0,
            color: [1.0, 0.0, 0.0, 1.0],
            line_width: 1.0,
        };
        let cloned = level.clone();
        assert_eq!(cloned.id, 1);
        assert_eq!(cloned.price, 150.0);
        // Debug should not panic.
        let _ = format!("{:?}", cloned);
    }

    #[test]
    fn level_serde_round_trip() {
        let level = HorizontalLevel {
            id: 42,
            price: 175.50,
            color: [0.0, 1.0, 0.5, 0.8],
            line_width: 2.0,
        };
        let json = serde_json::to_string(&level).expect("serialize");
        let decoded: HorizontalLevel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.id, 42);
        assert!((decoded.price - 175.50).abs() < f64::EPSILON);
        assert_eq!(decoded.line_width, 2.0);
    }
}
