//! `LineStyle`: dash pattern for horizontal-price annotations.
//!
//! Moved verbatim from `midas-chart/src/widget/level.rs` (Slice A1).
//! The data + constructors live here so `PriceLine` (which holds a
//! `LineStroke { style: LineStyle }`) does not need to reach back into
//! `midas-chart`.
//!
//! The line-rendering helper `segmented_line()` and the `compute_*`
//! entry points stay in `midas-chart` — they depend on chart-only
//! `GridLineInstance` GPU types and on `ComputeContext`.

use serde::{Deserialize, Serialize};
use smallvec::{smallvec, SmallVec};

/// Line rendering style.
///
/// `Pattern` holds an SVG-style `stroke-dasharray`: alternating on/off run
/// lengths in logical pixels, walked cyclically starting with an "on" run.
/// An empty pattern is equivalent to `Solid`. Dashed and dotted lines are
/// rendered as multiple short `GridLineInstance` segments; the GPU pipeline
/// still draws axis-aligned rectangles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum LineStyle {
    /// Continuous line.
    #[default]
    Solid,
    /// SVG-style dash pattern. Alternating on/off run lengths in logical
    /// pixels, walked cyclically. An empty pattern is equivalent to `Solid`.
    Pattern(SmallVec<[f32; 6]>),
}

impl LineStyle {
    /// 1-on / 3-off dotted rhythm.
    pub fn dotted() -> Self {
        Self::Pattern(smallvec![1.0, 3.0])
    }
    /// 1-on / 6-off sparse dotted rhythm.
    pub fn sparse_dotted() -> Self {
        Self::Pattern(smallvec![1.0, 6.0])
    }
    /// 6-on / 3-off dashed rhythm.
    pub fn dashed() -> Self {
        Self::Pattern(smallvec![6.0, 3.0])
    }
    /// 10-on / 4-off long-dash rhythm.
    pub fn dashed_long() -> Self {
        Self::Pattern(smallvec![10.0, 4.0])
    }
    /// 6-on / 3-off / 1-on / 3-off dash-dot rhythm.
    pub fn dash_dot() -> Self {
        Self::Pattern(smallvec![6.0, 3.0, 1.0, 3.0])
    }
    /// 6-on / 3-off / 1-on / 3-off / 1-on / 3-off dash-dot-dot rhythm.
    pub fn dash_dot_dot() -> Self {
        Self::Pattern(smallvec![6.0, 3.0, 1.0, 3.0, 1.0, 3.0])
    }

    /// True when this style draws as a single continuous segment.
    pub fn is_solid(&self) -> bool {
        match self {
            Self::Solid => true,
            Self::Pattern(p) => p.is_empty(),
        }
    }
}
