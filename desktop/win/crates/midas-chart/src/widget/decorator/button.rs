//! Single-shape decorator button with a glyph and optional hover fill.
//!
//! Buttons are simpler than badges: one shape, one glyph, fixed size. The
//! glyph is rendered by iced as an overlay label; the shape is rasterized by
//! the SDF badge pipeline in the same GPU draw call as everything else.

use super::badge::{BadgeBorder, BadgeShape};
use serde::{Deserialize, Serialize};

/// A clickable decorator with a single shape and a single glyph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Button {
    /// Geometric shape of the button body.
    pub shape: BadgeShape,
    /// Default fill color (linear RGBA).
    pub fill: [f32; 4],
    /// Fill color when the pointer is directly over this button. `None`
    /// means no hover state change.
    pub hover_fill: Option<[f32; 4]>,
    /// Glyph character rendered centered inside the button.
    pub glyph: char,
    /// RGBA color for the glyph.
    pub glyph_color: [f32; 4],
    /// Font size of the glyph in logical pixels.
    pub glyph_size: f32,
    /// Button body size in logical pixels, `[width, height]`.
    pub size: [f32; 2],
    /// Optional outline.
    pub border: Option<BadgeBorder>,
}
