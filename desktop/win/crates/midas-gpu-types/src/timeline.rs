//! Timeline-axis label metadata.
//!
//! `TimelineLabel` is the positioned-text record produced by the chart's
//! TC2000-style adaptive timeline algorithm in `midas-chart::timeline`.
//! It lives in this crate so that downstream consumers (e.g. the GPU
//! text-pipeline in `midas-render`) can depend on the type without
//! pulling the whole chart core.
//!
//! The companion `Tier` enum stays in `midas-chart::timeline` because it
//! is part of the algorithm-internal API surface; only the rendered
//! record is exported here.

/// Display tier: which date component is shown as the primary label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// "12:45 p", "1:00 p" — minute-level labels.
    Minute,
    /// "10 a", "1 p" — hour-level labels.
    Hour,
    /// "5", "12", "29" — day-of-month numbers.
    Day,
    /// "Jan", "Feb", "Mar" — month names.
    Month,
}

/// A single positioned timeline label for the time axis.
#[derive(Clone, Debug)]
pub struct TimelineLabel {
    /// Primary label text (e.g. "10 a", "5", "Jan").
    pub text: String,
    /// Boundary (secondary) text shown at tier transitions
    /// (e.g. "Mar 26, 2026" at a day boundary). `None` for regular labels.
    pub secondary: Option<String>,
    /// Screen X position in logical pixels.
    pub screen_x: f32,
    /// Whether this label sits at a higher-order boundary.
    pub is_boundary: bool,
    /// The display tier this label belongs to (Minute, Hour, Day, Month).
    pub tier: Tier,
}
