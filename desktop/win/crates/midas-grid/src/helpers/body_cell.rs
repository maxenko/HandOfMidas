//! `grid_body_cell` — per-cell body helper for hand-built panels.
//!
//! Every cell in the hand-built blotter and watchlist grids wants the
//! same chrome: fixed column width, `[4, 8]` padding, and — critically
//! — content clipped at the column boundary. Without clipping, a long
//! status string in a narrow "Status" column overflows into the next
//! column and gets painted over the following cell's text.
//!
//! Clipping is the default (and currently only) behaviour. If a caller
//! ever needs overflow, they can construct the container manually.

use iced::widget::container;
use iced::Element;

/// Default vertical / horizontal padding for a body cell, matching the
/// hand-rolled values the blotter used before the helper landed.
pub const BODY_CELL_PADDING: [u16; 2] = [4, 8];

/// Wrap a cell `content` element in the standard body-cell container.
///
/// - fixed `width` (the grid's column width);
/// - `[4, 8]` padding;
/// - `clip(true)` so content never bleeds into the next column.
pub fn grid_body_cell<'a, M: 'a>(content: Element<'a, M>, width: f32) -> Element<'a, M> {
    container(content)
        .width(width)
        .padding(BODY_CELL_PADDING)
        .clip(true)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[test]
    fn builds_without_panic() {
        let inner: Element<'_, ()> = text("cell").into();
        let _el = grid_body_cell(inner, 100.0);
    }
}
