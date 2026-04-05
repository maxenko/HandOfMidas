//! Body row layout and rendering.

use std::collections::HashMap;

use iced::widget::{container, mouse_area, Column, Row, Space};
use iced::{Color, Element, Fill};

use crate::column::{Alignment, ColumnId, GridColumn};
use crate::message::GridMessage;
use crate::state::GridState;
use crate::style;

/// Build the body rows for a grid.
///
/// Iterates `rows`, builds one `Row` per data row using `col.cell()`.
/// Cell widgets emit `M` directly. Row selection areas call `on_grid`.
pub fn grid_body<'a, T, M, C>(
    rows: &'a [T],
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

    let grid_style = style::GridStyle::default();
    let selected_bg = grid_style.selected_bg;
    let row_height = grid_style.row_height;

    let mut body = Column::new();

    if rows.is_empty() {
        return body.into();
    }

    for (row_idx, row_data) in rows.iter().enumerate() {
        let is_selected = state.selection.is_selected(row_idx);
        let row_bg = if is_selected {
            selected_bg
        } else {
            Color::TRANSPARENT
        };

        let mut cells: Vec<Element<'a, M>> = Vec::with_capacity(col_order.len() * 2);
        for (col_idx, &col_id) in col_order.iter().enumerate() {
            let Some(&idx) = col_index.get(&col_id) else {
                continue;
            };
            let col = &columns[idx];
            let width = state.column_width(col_id);
            let cell_content = col.cell(row_data, row_idx);
            let h_align = match col.align() {
                Alignment::Start => iced::alignment::Horizontal::Left,
                Alignment::Center => iced::alignment::Horizontal::Center,
                Alignment::End => iced::alignment::Horizontal::Right,
            };

            cells.push(
                container(cell_content)
                    .width(width)
                    .height(row_height)
                    .padding([2, 4])
                    .align_x(h_align)
                    .align_y(iced::alignment::Vertical::Center)
                    .clip(true)
                    .style(|_| container::Style {
                        border: iced::Border {
                            color: style::GRID_BORDER_COLOR,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..Default::default()
                    })
                    .into(),
            );

            if col_idx < col_order.len() - 1 {
                cells.push(Space::new().width(4).height(Fill).into());
            }
        }

        let inner_row = Row::with_children(cells)
            .padding([0, 4])
            .align_y(iced::Alignment::Center);

        let msg = on_grid(GridMessage::RowSelected(row_idx));
        let selectable_row = mouse_area(
            container(inner_row).style(move |_| container::Style {
                background: Some(row_bg.into()),
                ..Default::default()
            }),
        )
        .on_release(msg);

        body = body.push(selectable_row);
    }

    body.into()
}
