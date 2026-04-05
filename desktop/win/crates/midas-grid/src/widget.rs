//! Grid builder and composition.
//!
//! Phase 0-1: `grid()` returns an `Element` built from composition functions.
//! Phase 2: replaced by a custom `Widget<M>` impl.

use iced::widget::{column, scrollable};
use iced::{Element, Fill};

use crate::body::grid_body;
use crate::column::GridColumn;
use crate::header::grid_header;
use crate::message::GridMessage;
use crate::state::GridState;

/// Build a grid widget.
///
/// `on_grid` is a required parameter that maps grid chrome events
/// (sort, resize, select, drag) to the application's message type.
/// Cell content emits `M` directly; only grid chrome routes through
/// the callback.
///
/// ```ignore
/// grid(&columns, &rows, &grid_state, move |gm| Message::WatchlistGrid(wl_id, gm))
/// ```
pub fn grid<'a, T, M, C>(
    columns: &'a [C],
    rows: &'a [T],
    state: &'a GridState,
    on_grid: impl Fn(GridMessage) -> M + 'a,
) -> Grid<'a, T, M, C>
where
    C: GridColumn<T, M>,
    M: Clone + 'a,
{
    Grid {
        columns,
        rows,
        state,
        on_grid: Box::new(on_grid),
    }
}

/// Grid builder. Constructed via [`grid()`].
pub struct Grid<'a, T, M, C> {
    columns: &'a [C],
    rows: &'a [T],
    state: &'a GridState,
    on_grid: Box<dyn Fn(GridMessage) -> M + 'a>,
}

impl<'a, T, M, C> From<Grid<'a, T, M, C>> for Element<'a, M>
where
    C: GridColumn<T, M>,
    M: Clone + 'a,
{
    fn from(g: Grid<'a, T, M, C>) -> Self {
        let header = grid_header(g.columns, g.state, &*g.on_grid);
        let body = grid_body(g.rows, g.columns, g.state, &*g.on_grid);

        column![header, scrollable(body).height(Fill)].into()
    }
}
