//! Marker annotation type.
//!
//! Moved from `midas-chart/src/widget/marker.rs` (Slice A1) — required
//! here because `AnnotationKind::Marker(MarkerAnnotation)` is a variant
//! of the moved enum. This is pure data with no chart-only deps.
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
