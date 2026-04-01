//! Horizontal level annotation type.
//!
//! A `HorizontalLevel` represents a user-drawn horizontal line at a specific
//! price. This is the new widget-system version that adds `LineStyle` and
//! `LevelExtend` capabilities. The existing `levels.rs::HorizontalLevel`
//! is retained during migration and deprecated after Phase 1B.

use crate::levels::LevelIcon;
use serde::{Deserialize, Serialize};

/// A horizontal line at a specific price.
///
/// The most common annotation type. Represents support/resistance levels,
/// moving average values, or any price of interest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HorizontalLevel {
    /// Price at which the horizontal line is drawn.
    pub price: f64,
    /// RGBA color of the line (linear space, NOT sRGB).
    pub color: [f32; 4],
    /// Line width in logical pixels. Typical: 1.0-3.0.
    pub line_width: f32,
    /// Line rendering style (solid, dashed, dotted).
    pub style: LineStyle,
    /// Optional text label displayed next to the price on the Y axis.
    pub label: Option<String>,
    /// How far the line extends horizontally.
    pub extend: LevelExtend,
    /// Icon displayed next to the label.
    pub icon: LevelIcon,
}

/// Line rendering style.
///
/// Dashed and dotted lines are rendered as multiple short `GridLineInstance`
/// segments. The GPU pipeline is unchanged -- it still draws axis-aligned
/// rectangles. The segmentation happens in the compute phase.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    /// Continuous line.
    #[default]
    Solid,
    /// Alternating dash/gap segments.
    Dashed {
        /// Length of each dash segment in logical pixels.
        dash_len: f32,
        /// Length of each gap between dashes in logical pixels.
        gap_len: f32,
    },
    /// Regularly spaced dots.
    Dotted {
        /// Spacing between dot centers in logical pixels.
        dot_spacing: f32,
    },
}

/// How far a level line extends horizontally across the chart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LevelExtend {
    /// Spans the entire visible chart width. Most common.
    #[default]
    FullWidth,
    /// Starts at a specific time, extends infinitely to the right.
    RightFrom {
        /// Epoch milliseconds at which the line starts.
        timestamp: i64,
    },
    /// Bounded segment between two timestamps.
    Between {
        /// Start timestamp (epoch ms).
        start: i64,
        /// End timestamp (epoch ms).
        end: i64,
    },
}
