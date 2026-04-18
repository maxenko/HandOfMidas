//! Grid style constants and helpers.
//!
//! Migrated from `views.rs` `GRID_BORDER_COLOR` / `GRID_HEADER_BORDER_COLOR`.

use iced::Color;

/// Border color for data cells.
pub const GRID_BORDER_COLOR: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.06);

/// Border color for header cells (slightly more visible).
pub const GRID_HEADER_BORDER_COLOR: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.12);

/// Background tint applied to alternating body rows (even-indexed rows).
///
/// Used by the `helpers::grid_body_row` renderer when `alt_bg: true` and
/// the row is not selected. Matches the blotter's historical inline value.
pub const ALT_ROW_BG: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.02);

/// Configurable style parameters for a grid instance.
#[derive(Debug, Clone)]
pub struct GridStyle {
    /// Border color for header cells.
    pub header_border_color: Color,
    /// Border color for data cells.
    pub cell_border_color: Color,
    /// Row height in logical pixels.
    pub row_height: f32,
    /// Header height in logical pixels.
    pub header_height: f32,
    /// Background color for selected rows.
    pub selected_bg: Color,
    /// Background color for hovered rows (Phase 1+).
    pub hover_bg: Color,
    /// Width of the resize handle hit zone in logical pixels.
    pub resize_handle_width: f32,
}

impl Default for GridStyle {
    fn default() -> Self {
        Self {
            header_border_color: GRID_HEADER_BORDER_COLOR,
            cell_border_color: GRID_BORDER_COLOR,
            row_height: 28.0,
            header_height: 26.0,
            selected_bg: Color::from_rgba(0.2, 0.35, 0.55, 0.6),
            hover_bg: Color::from_rgba(1.0, 1.0, 1.0, 0.04),
            resize_handle_width: 4.0,
        }
    }
}
