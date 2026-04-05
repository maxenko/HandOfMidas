//! Header row layout and rendering.

use std::collections::HashMap;

use iced::widget::{container, mouse_area, row, text, Row, Space};
use iced::{Element, Fill};

use crate::column::{Alignment, ColumnId, GridColumn};
use crate::message::GridMessage;
use crate::state::GridState;
use crate::style;

/// Build the header row for a grid.
///
/// Iterates columns in display order, calls `col.header()` for each,
/// composites sort indicators, and interleaves resize handles.
pub fn grid_header<'a, T, M, C>(
    columns: &'a [C],
    state: &GridState,
    on_grid: &dyn Fn(GridMessage) -> M,
) -> Element<'a, M>
where
    C: GridColumn<T, M>,
    M: Clone + 'a,
{
    let col_order = state.effective_order(columns);

    let col_index: HashMap<ColumnId, usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id(), i))
        .collect();

    let header_height = style::GridStyle::default().header_height;

    let mut header_cells: Vec<Element<'a, M>> = Vec::with_capacity(col_order.len() * 2);

    for (i, &col_id) in col_order.iter().enumerate() {
        let Some(&idx) = col_index.get(&col_id) else {
            continue;
        };
        let col = &columns[idx];

        let width = state.column_width(col_id);
        let h_align = match col.align() {
            Alignment::Start => iced::alignment::Horizontal::Left,
            Alignment::Center => iced::alignment::Horizontal::Center,
            Alignment::End => iced::alignment::Horizontal::Right,
        };

        // Build header content with optional sort indicator.
        let header_content: Element<'a, M> = if col.sortable() {
            let sort_indicator = state
                .sort
                .filter(|s| s.column_id == col_id)
                .map(|s| s.direction.indicator())
                .unwrap_or("");

            let msg = on_grid(GridMessage::SortToggled(col_id));
            mouse_area(
                container(row![col.header(), text(sort_indicator).size(12)])
                    .width(width)
                    .height(header_height)
                    .padding([2, 4])
                    .align_x(h_align)
                    .align_y(iced::alignment::Vertical::Center)
                    .clip(true)
                    .style(|_| container::Style {
                        border: iced::Border {
                            color: style::GRID_HEADER_BORDER_COLOR,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .on_release(msg)
            .into()
        } else {
            container(col.header())
                .width(width)
                .height(header_height)
                .padding([2, 4])
                .align_x(h_align)
                .align_y(iced::alignment::Vertical::Center)
                .clip(true)
                .style(|_| container::Style {
                    border: iced::Border {
                        color: style::GRID_HEADER_BORDER_COLOR,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        };

        header_cells.push(header_content);

        // Interleave 4px resize handle between columns (Phase 0 width).
        if i < col_order.len() - 1 {
            header_cells.push(
                Space::new()
                    .width(4)
                    .height(Fill)
                    .into(),
            );
        }
    }

    Row::with_children(header_cells)
        .padding([0, 4])
        .into()
}
