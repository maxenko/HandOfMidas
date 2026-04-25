//! `PriceLine`: the shared geometric primitive for every horizontal-price
//! annotation (levels, bracket legs, future alert lines).
//!
//! Moved verbatim from `midas-chart/src/widget/price_line.rs` (Slice A1).
//! Decorators are attached by the wrapping domain type, not stored on the
//! `PriceLine` itself — the primitive stays independent of its visual
//! accessories.

use crate::line_style::LineStyle;
use serde::{Deserialize, Serialize};

/// A horizontal line at a specific price.
///
/// This is the canonical geometry for any annotation that renders as a
/// horizontal stroke. Color, width, and dash pattern live inside
/// `stroke: LineStroke`; the time-axis footprint lives inside `extent`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PriceLine {
    /// Price the line is drawn at.
    pub price: f64,
    /// Time-axis footprint.
    pub extent: LineExtent,
    /// Color, width, and dash pattern.
    pub stroke: LineStroke,
}

/// Color, width, and dash style for a `PriceLine`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineStroke {
    /// Linear RGBA (not sRGB).
    pub color: [f32; 4],
    /// Stroke width in logical pixels.
    pub width: f32,
    /// Dash pattern; `Solid` or `Pattern(empty)` draws a continuous line.
    pub style: LineStyle,
}

/// Time-axis footprint for a `PriceLine`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineExtent {
    /// Spans the entire visible chart width. Default, matches current
    /// level behavior.
    #[default]
    FullWidth,
    /// Starts at a specific time and extends infinitely to the right.
    /// Used for order-bracket legs pinned to an order-open timestamp.
    RightFrom {
        /// Epoch milliseconds at which the line starts.
        timestamp: i64,
    },
    /// Bounded segment between two timestamps. Reserved for time-limited
    /// alerts and per-bracket bounds.
    Between {
        /// Epoch ms start.
        start: i64,
        /// Epoch ms end.
        end: i64,
    },
}
