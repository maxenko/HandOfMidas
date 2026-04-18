//! `grid_body_row` — per-row body helper for hand-built panels.
//!
//! Per-cell/per-row, for hand-built panels. The `grid()` builder uses
//! [`crate::header::grid_header`] / [`crate::body::grid_body`] instead.

use iced::widget::{container, mouse_area, Row};
use iced::{Color, Element, Fill};

use crate::style::{GridStyle, ALT_ROW_BG};

/// Build a single body row from pre-composed cells.
///
/// Semantics:
/// - Background resolves from the default [`GridStyle`]:
///   `selected → GridStyle::default().selected_bg`,
///   else `alt_bg → ALT_ROW_BG`,
///   else `Color::TRANSPARENT`.
/// - When `on_click.is_some()`, wraps the row in a `mouse_area` that
///   emits the click message on mouse-release.
///
/// The caller owns cell construction — the helper only wraps the row
/// with the correct background + click binding.
pub fn grid_body_row<'a, M: Clone + 'a>(
    cells: Vec<Element<'a, M>>,
    selected: bool,
    alt_bg: bool,
    on_click: Option<M>,
) -> Element<'a, M> {
    let style = GridStyle::default();
    let bg = if selected {
        style.selected_bg
    } else if alt_bg {
        ALT_ROW_BG
    } else {
        Color::TRANSPARENT
    };

    let inner = Row::with_children(cells)
        .padding([0, 4])
        .align_y(iced::Alignment::Center);

    let row_container = container(inner)
        .width(Fill)
        .style(move |_| container::Style {
            background: Some(bg.into()),
            ..Default::default()
        });

    match on_click {
        Some(msg) => mouse_area(row_container).on_release(msg).into(),
        None => row_container.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestMsg {
        Click,
    }

    fn cells<'a>() -> Vec<Element<'a, TestMsg>> {
        vec![text("A").into(), text("B").into()]
    }

    #[test]
    fn builds_selected_no_click() {
        let _el: Element<'_, TestMsg> = grid_body_row(cells(), true, false, None);
    }

    #[test]
    fn builds_alt_bg_with_click() {
        let _el: Element<'_, TestMsg> = grid_body_row(cells(), false, true, Some(TestMsg::Click));
    }

    #[test]
    fn builds_plain_no_click() {
        let _el: Element<'_, TestMsg> = grid_body_row(cells(), false, false, None);
    }

    #[test]
    fn builds_selected_and_alt_bg() {
        // Selected should win over alt_bg when both are true — the
        // helper just resolves to `selected_bg` without panicking.
        let _el: Element<'_, TestMsg> = grid_body_row(cells(), true, true, Some(TestMsg::Click));
    }

    #[test]
    fn builds_with_empty_cells() {
        let empty: Vec<Element<'_, TestMsg>> = vec![];
        let _el: Element<'_, TestMsg> = grid_body_row(empty, false, false, None);
    }
}
