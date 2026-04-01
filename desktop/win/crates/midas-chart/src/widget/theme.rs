//! Widget theme: default colors for annotations and indicators.
//!
//! Annotations can override these with per-annotation colors.
//! These are the fallback defaults when an annotation doesn't
//! specify its own color.

/// Theme colors used by default annotation styling.
#[derive(Clone, Debug)]
pub struct Theme {
    /// Default level line color.
    pub level_color: [f32; 4],
    /// Default long bracket color (green-ish).
    pub bracket_long_color: [f32; 4],
    /// Default short bracket color (red-ish).
    pub bracket_short_color: [f32; 4],
    /// Default take-profit color.
    pub bracket_tp_color: [f32; 4],
    /// Default stop-loss color.
    pub bracket_sl_color: [f32; 4],
    /// Default bracket zone fill alpha.
    pub bracket_zone_alpha: f32,
    /// Default note background color.
    pub note_bg_color: [f32; 4],
    /// Default note text color.
    pub note_text_color: [f32; 4],
    /// Default marker color.
    pub marker_color: [f32; 4],
    /// Selection highlight color (glow around selected annotations).
    pub selection_color: [f32; 4],
    /// Selection highlight extra thickness in logical pixels.
    pub selection_thickness: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            level_color: [0.0, 0.7, 1.0, 0.9],
            bracket_long_color: [0.15, 0.65, 0.35, 0.9],
            bracket_short_color: [0.65, 0.15, 0.15, 0.9],
            bracket_tp_color: [0.15, 0.65, 0.35, 0.5],
            bracket_sl_color: [0.65, 0.15, 0.15, 0.5],
            bracket_zone_alpha: 0.06,
            note_bg_color: [0.15, 0.15, 0.2, 0.85],
            note_text_color: [0.9, 0.9, 0.9, 1.0],
            marker_color: [0.8, 0.8, 0.2, 0.9],
            selection_color: [1.0, 1.0, 1.0, 0.4],
            selection_thickness: 2.0,
        }
    }
}
