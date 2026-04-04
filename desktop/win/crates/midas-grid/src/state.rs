//! Grid UI state — owned by the application, passed to the grid by reference.

use std::collections::HashMap;

use crate::column::{ColumnId, SortDirection, SortSpec};

/// Complete UI state for one grid instance.
///
/// Owned by the application (e.g. inside `WatchlistPanel`). Passed by
/// reference to the grid widget during `view()`. Mutated only in the
/// application's `update()` via grid messages.
///
/// **NOT `Serialize`/`Deserialize`**: `ColumnId(&'static str)` cannot be
/// deserialized as a `HashMap` key. Use `to_config()` / `from_config()`
/// for persistence.
#[derive(Debug, Clone)]
pub struct GridState {
    /// Ordered list of column IDs defining display order.
    pub column_order: Vec<ColumnId>,

    /// Column widths keyed by column ID (logical pixels).
    pub column_widths: HashMap<ColumnId, f32>,

    /// Active sort specification, or `None` for no sorting.
    pub sort: Option<SortSpec>,

    /// Row selection state (Phase 0: single selection only).
    pub selection: SelectionState,

    /// Vertical scroll offset (logical pixels).
    pub scroll_y: f32,

    /// Active drag/resize interaction, if any.
    pub interaction: ActiveInteraction,
}

impl GridState {
    /// Create a new grid state with the given column order and widths.
    pub fn new(column_order: Vec<ColumnId>, column_widths: HashMap<ColumnId, f32>) -> Self {
        Self {
            column_order,
            column_widths,
            sort: None,
            selection: SelectionState::default(),
            scroll_y: 0.0,
            interaction: ActiveInteraction::None,
        }
    }

    /// Get the resolved width for a column, or `default` if not set.
    pub fn column_width(&self, id: ColumnId) -> f32 {
        self.column_widths.get(&id).copied().unwrap_or(80.0)
    }

    /// Set a column's width, clamping to `[min, max]`.
    pub fn set_column_width(&mut self, id: ColumnId, width: f32, min: f32, max: Option<f32>) {
        let clamped = if let Some(max) = max {
            width.clamp(min, max)
        } else {
            width.max(min)
        };
        self.column_widths.insert(id, clamped);
    }

    /// Phase 0 two-state sort toggle (Asc <-> Desc).
    ///
    /// If the same column is clicked, flip direction.
    /// If a different column, start with `default_direction`.
    /// The `default_direction` parameter is provided by the app (not the grid)
    /// to keep the grid crate generic.
    pub fn toggle_sort(&mut self, column: ColumnId, default_direction: SortDirection) {
        self.sort = match self.sort {
            Some(spec) if spec.column_id == column => Some(SortSpec {
                column_id: column,
                direction: spec.direction.toggle(),
            }),
            _ => Some(SortSpec {
                column_id: column,
                direction: default_direction,
            }),
        };
    }

    /// Effective column order: uses `column_order` if non-empty, otherwise
    /// falls back to definition order from the provided columns.
    pub fn effective_order<T, M, C: crate::column::GridColumn<T, M>>(&self, columns: &[C]) -> Vec<ColumnId> {
        if self.column_order.is_empty() {
            columns.iter().map(|c| c.id()).collect()
        } else {
            self.column_order.clone()
        }
    }

    /// Move a column from one display position to another.
    ///
    /// After removal, the column is inserted at index `to` in the
    /// resulting (shorter) vector. When `from < to`, the effective
    /// position is one past the original `to` element.
    pub fn move_column(&mut self, from: usize, to: usize) {
        if from < self.column_order.len() && to < self.column_order.len() && from != to {
            let col = self.column_order.remove(from);
            self.column_order.insert(to, col);
        }
    }
}

/// Row selection state (Phase 0: single selection only).
///
/// Phase 3a replaces this with `BTreeSet<RowKey>` for multi-selection.
#[derive(Debug, Clone, Default)]
pub struct SelectionState {
    /// Index of the selected row, or `None`.
    pub selected: Option<usize>,
    /// Index of the focused row (for keyboard nav), or `None`.
    pub focused: Option<usize>,
}

impl SelectionState {
    /// Select a single row by index.
    pub fn select(&mut self, index: usize) {
        self.selected = Some(index);
        self.focused = Some(index);
    }

    /// Check if a row is selected.
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected == Some(index)
    }

    /// Clear the selection.
    pub fn clear(&mut self) {
        self.selected = None;
        self.focused = None;
    }
}

/// Unified interaction state. Only one drag/resize interaction can be
/// active at a time.
///
/// Phase 0: only `None` exists.
/// Phase 1: adds `Resize(ResizeState)`.
/// Phase 2: adds `ColumnDrag` and `RowDrag`.
#[derive(Debug, Clone, Default)]
pub enum ActiveInteraction {
    #[default]
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::SortDirection;

    fn test_state() -> GridState {
        let order = vec![
            ColumnId("drag"),
            ColumnId("fav"),
            ColumnId("ticker"),
            ColumnId("price"),
        ];
        let mut widths = HashMap::new();
        widths.insert(ColumnId("drag"), 26.0);
        widths.insert(ColumnId("fav"), 30.0);
        widths.insert(ColumnId("ticker"), 70.0);
        widths.insert(ColumnId("price"), 80.0);
        GridState::new(order, widths)
    }

    #[test]
    fn column_width_returns_stored_value() {
        let state = test_state();
        assert_eq!(state.column_width(ColumnId("ticker")), 70.0);
    }

    #[test]
    fn column_width_returns_default_for_unknown() {
        let state = test_state();
        assert_eq!(state.column_width(ColumnId("unknown")), 80.0);
    }

    #[test]
    fn set_column_width_clamps_to_min() {
        let mut state = test_state();
        state.set_column_width(ColumnId("ticker"), 5.0, 20.0, None);
        assert_eq!(state.column_width(ColumnId("ticker")), 20.0);
    }

    #[test]
    fn set_column_width_clamps_to_max() {
        let mut state = test_state();
        state.set_column_width(ColumnId("ticker"), 500.0, 20.0, Some(200.0));
        assert_eq!(state.column_width(ColumnId("ticker")), 200.0);
    }

    #[test]
    fn toggle_sort_new_column() {
        let mut state = test_state();
        state.toggle_sort(ColumnId("price"), SortDirection::Descending);
        let sort = state.sort.unwrap();
        assert_eq!(sort.column_id, ColumnId("price"));
        assert_eq!(sort.direction, SortDirection::Descending);
    }

    #[test]
    fn toggle_sort_same_column_flips() {
        let mut state = test_state();
        state.toggle_sort(ColumnId("price"), SortDirection::Descending);
        state.toggle_sort(ColumnId("price"), SortDirection::Descending);
        let sort = state.sort.unwrap();
        assert_eq!(sort.direction, SortDirection::Ascending);
    }

    #[test]
    fn toggle_sort_different_column_resets() {
        let mut state = test_state();
        state.toggle_sort(ColumnId("price"), SortDirection::Descending);
        state.toggle_sort(ColumnId("ticker"), SortDirection::Ascending);
        let sort = state.sort.unwrap();
        assert_eq!(sort.column_id, ColumnId("ticker"));
        assert_eq!(sort.direction, SortDirection::Ascending);
    }

    #[test]
    fn selection_single() {
        let mut sel = SelectionState::default();
        assert!(!sel.is_selected(0));
        sel.select(2);
        assert!(sel.is_selected(2));
        assert!(!sel.is_selected(0));
    }

    #[test]
    fn selection_clear() {
        let mut sel = SelectionState::default();
        sel.select(1);
        sel.clear();
        assert!(!sel.is_selected(1));
        assert_eq!(sel.selected, None);
    }

    #[test]
    fn move_column() {
        let mut state = test_state();
        state.move_column(0, 2);
        assert_eq!(state.column_order[0], ColumnId("fav"));
        assert_eq!(state.column_order[1], ColumnId("ticker"));
        assert_eq!(state.column_order[2], ColumnId("drag"));
    }

    #[test]
    fn move_column_same_position_is_noop() {
        let mut state = test_state();
        let original = state.column_order.clone();
        state.move_column(1, 1);
        assert_eq!(state.column_order, original);
    }

    #[test]
    fn move_column_out_of_bounds_is_noop() {
        let mut state = test_state();
        let original = state.column_order.clone();
        state.move_column(10, 0);
        assert_eq!(state.column_order, original);
    }
}
