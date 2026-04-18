//! `grid_header_cell` — per-cell header helper for hand-built panels.
//!
//! Per-cell/per-row, for hand-built panels. The `grid()` builder uses
//! [`crate::header::grid_header`] / [`crate::body::grid_body`] instead.

use iced::alignment::Horizontal;
use iced::widget::{container, mouse_area, row, stack, text, Space};
use iced::{Color, Element, Fill};

use crate::style::GRID_HEADER_BORDER_COLOR;

use super::ResizeHandle;

/// Style parameters for a header cell.
///
/// Mirrors the existing [`crate::style::GridStyle`] convention. The
/// default matches the blotter's current inline values so the blotter
/// migration can pass `HeaderStyle::default()`. The watchlist constructs
/// a custom style explicitly at the call site (`[2, 4]` padding, `1.0`
/// border width, larger font, no muted colour).
#[derive(Debug, Clone)]
pub struct HeaderStyle {
    /// `[vertical, horizontal]` padding inside the cell, logical px.
    pub padding: [u16; 2],
    /// Border width, logical px.
    pub border_width: f32,
    /// Border colour.
    pub border_color: Color,
    /// Font size for label and sort indicator text.
    pub label_size: u16,
    /// Optional text colour for label/indicator; `None` = theme default.
    pub label_color: Option<Color>,
    /// Optional horizontal alignment of the label within the cell.
    /// `None` leaves the label at the start of the padding box
    /// (existing behaviour); `Some(Center)` is used for glyph-only
    /// columns like the watchlist favourite star.
    pub align_x: Option<Horizontal>,
}

impl Default for HeaderStyle {
    fn default() -> Self {
        // Blotter values.
        Self {
            padding: [6, 8],
            border_width: 0.5,
            border_color: GRID_HEADER_BORDER_COLOR,
            label_size: 11,
            label_color: None,
            align_x: None,
        }
    }
}

/// Build a single header cell.
///
/// Composes:
/// - label + optional sort indicator, wrapped in `mouse_area.on_release`
///   when `sort_msg.is_some()`;
/// - optional 4-px right-edge resize strip layered via `stack!` (so it
///   does not push cell width) when `resize.is_some()`.
///
/// When both `sort_msg` and `resize` are `None`, returns a plain
/// container cell (no interactivity).
pub fn grid_header_cell<'a, M: Clone + 'a>(
    label: &'a str,
    width: f32,
    sort_indicator: &'a str,
    sort_msg: Option<M>,
    resize: Option<ResizeHandle<M>>,
    style: &HeaderStyle,
) -> Element<'a, M> {
    // Capture style into owned values so the container style closure
    // doesn't borrow `style` (which lives only for the call duration).
    let border_color = style.border_color;
    let border_width = style.border_width;
    let padding = style.padding;
    let label_size = style.label_size;
    let label_color = style.label_color;

    let mut label_text = text(label).size(label_size as f32);
    if let Some(color) = label_color {
        label_text = label_text.color(color);
    }
    // Only include the indicator slot when there is actually an indicator
    // to show. An empty `text("")` plus `spacing(4)` pushes the label
    // ~2px off-centre in centred cells (e.g. the favourite-star column).
    let label_inner: Element<'a, M> = if sort_indicator.is_empty() {
        label_text.into()
    } else {
        let mut indicator_text = text(sort_indicator).size(label_size as f32);
        if let Some(color) = label_color {
            indicator_text = indicator_text.color(color);
        }
        row![label_text, indicator_text].spacing(4).into()
    };

    let mut cell_container = container(label_inner)
        .width(width)
        .padding(padding)
        .clip(true)
        .style(move |_| container::Style {
            border: iced::Border {
                color: border_color,
                width: border_width,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    if let Some(h) = style.align_x {
        cell_container = cell_container.align_x(h);
    }

    let header_content: Element<'a, M> = if let Some(msg) = sort_msg {
        mouse_area(cell_container).on_release(msg).into()
    } else {
        cell_container.into()
    };

    match resize {
        Some(handle) => {
            let resize_handle = container(
                mouse_area(Space::new().width(4).height(handle.height))
                    .interaction(iced::mouse::Interaction::ResizingHorizontally)
                    .on_press(handle.on_press),
            )
            .width(Fill)
            .align_x(iced::alignment::Horizontal::Right);

            stack![header_content, resize_handle].width(width).into()
        }
        None => header_content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TestMsg {
        Sort,
        ResizeStart,
    }

    #[test]
    fn builds_sortable_with_resize() {
        let style = HeaderStyle::default();
        let _el: Element<'_, TestMsg> = grid_header_cell(
            "Price",
            100.0,
            " \u{25B2}",
            Some(TestMsg::Sort),
            Some(ResizeHandle {
                on_press: TestMsg::ResizeStart,
                height: 26.0,
            }),
            &style,
        );
    }

    #[test]
    fn builds_sortable_no_resize() {
        let style = HeaderStyle::default();
        let _el: Element<'_, TestMsg> =
            grid_header_cell("Price", 100.0, "", Some(TestMsg::Sort), None, &style);
    }

    #[test]
    fn builds_non_sortable_with_resize() {
        let style = HeaderStyle::default();
        let _el: Element<'_, TestMsg> = grid_header_cell(
            "",
            30.0,
            "",
            None,
            Some(ResizeHandle {
                on_press: TestMsg::ResizeStart,
                height: 26.0,
            }),
            &style,
        );
    }

    #[test]
    fn builds_non_sortable_no_resize() {
        let style = HeaderStyle::default();
        let _el: Element<'_, TestMsg> = grid_header_cell("Symbol", 80.0, "", None, None, &style);
    }

    #[test]
    fn builds_with_watchlist_style() {
        // Watchlist uses [2, 4] padding + 1.0 border — ensure the helper
        // accepts the non-default style without panicking.
        let style = HeaderStyle {
            padding: [2, 4],
            border_width: 1.0,
            border_color: GRID_HEADER_BORDER_COLOR,
            label_size: 12,
            label_color: None,
            align_x: None,
        };
        let _el: Element<'_, TestMsg> =
            grid_header_cell("Ticker", 70.0, "", Some(TestMsg::Sort), None, &style);
    }
}
