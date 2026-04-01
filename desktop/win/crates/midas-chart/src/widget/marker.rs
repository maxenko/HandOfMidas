//! Marker annotation type.
//!
//! An icon or stamp at a specific price/time on the chart. Used for
//! fill markers, buy/sell signals, alerts, bookmarks, important events.

use serde::{Deserialize, Serialize};

/// An icon or stamp at a specific price/time on the chart.
///
/// Used for: fill markers, buy/sell signals, alerts, bookmarks,
/// important events. Rendered as small colored shapes via the GPU
/// pipeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkerAnnotation {
    /// Anchor price (Y position).
    pub price: f64,
    /// Anchor timestamp (X position), epoch milliseconds.
    pub timestamp: i64,
    /// Which icon shape to render.
    pub icon: MarkerIcon,
    /// Icon color.
    pub color: [f32; 4],
    /// Icon diameter in logical pixels. Typical: 6.0-16.0.
    pub size: f32,
    /// Tooltip text shown on hover. None = no tooltip.
    pub tooltip: Option<String>,
}

/// Available marker icon shapes.
///
/// Initially rendered as colored rectangles. Can be upgraded to
/// SDF-based shapes via a MarkerPipeline later without changing
/// the data model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerIcon {
    /// Filled circle. Used for fill events and generic markers.
    Circle,
    /// Upward-pointing triangle. Used for buy signals.
    TriangleUp,
    /// Downward-pointing triangle. Used for sell signals.
    TriangleDown,
    /// Diamond shape. Used for alerts.
    Diamond,
    /// X mark. Used for stop/cancel events.
    Cross,
    /// Flag shape. Used for important events.
    Flag,
    /// Star shape. Used for bookmarks.
    Star,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip() {
        let marker = MarkerAnnotation {
            price: 185.50,
            timestamp: 1700000000000,
            icon: MarkerIcon::TriangleUp,
            color: [0.0, 1.0, 0.0, 1.0],
            size: 10.0,
            tooltip: Some("Buy fill @ 185.50".into()),
        };
        let json = serde_json::to_string(&marker).expect("serialize");
        let decoded: MarkerAnnotation = serde_json::from_str(&json).expect("deserialize");
        assert!((decoded.price - 185.50).abs() < f64::EPSILON);
        assert_eq!(decoded.icon, MarkerIcon::TriangleUp);
        assert_eq!(decoded.tooltip.as_deref(), Some("Buy fill @ 185.50"));
    }

    #[test]
    fn all_icons_serialize() {
        let icons = [
            MarkerIcon::Circle,
            MarkerIcon::TriangleUp,
            MarkerIcon::TriangleDown,
            MarkerIcon::Diamond,
            MarkerIcon::Cross,
            MarkerIcon::Flag,
            MarkerIcon::Star,
        ];
        for icon in icons {
            let json = serde_json::to_string(&icon).expect("serialize");
            let decoded: MarkerIcon = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, icon);
        }
    }
}
