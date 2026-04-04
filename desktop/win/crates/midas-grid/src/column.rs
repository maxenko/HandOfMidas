//! Column trait, identifiers, and sort types for the grid component.

use iced::Element;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a column within a grid instance.
///
/// Uses `&'static str` for zero-cost `Copy` + human-readable TOML
/// serialization via `Display`. See 03-column-data-model.md §1.1.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct ColumnId(pub &'static str);

impl fmt::Display for ColumnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// How a column's width is determined.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ColumnWidth {
    /// Exact pixel width, not affected by layout.
    Fixed(f32),
    /// Proportional share of remaining space after Fixed columns.
    Flex(f32),
    /// Size to content (deferred to Phase 4).
    Auto,
}

/// Sort direction for a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    /// Return the opposite direction.
    pub fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    /// Unicode arrow indicator for header display.
    pub fn indicator(self) -> &'static str {
        match self {
            Self::Ascending => " \u{25B2}",
            Self::Descending => " \u{25BC}",
        }
    }
}

/// Which column is sorted and in what direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub column_id: ColumnId,
    pub direction: SortDirection,
}

/// Horizontal alignment of cell content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Alignment {
    #[default]
    Start,
    Center,
    End,
}

/// Core trait for defining grid columns.
///
/// Implement this for an enum of column variants to define how each
/// column renders its header and cells. The grid is generic over `T`
/// (row data type) and `M` (application message type).
///
/// **Phase 0 wiring**: `id()`, `header()`, `cell()`, `sortable()`,
/// `compare()`, and `align()` are used by the rendering pipeline.
/// `width()`, `min_width()`, `max_width()`, `resizable()`, and
/// `reorderable()` are defined for implementors but not yet consumed
/// by the grid layout — column widths come from `GridState::column_widths`.
/// These methods will be wired in during Phase 1-2.
pub trait GridColumn<T, M> {
    /// Stable identifier for this column.
    fn id(&self) -> ColumnId;

    /// Header content (label only — grid composites sort indicators).
    fn header(&self) -> Element<'_, M>;

    /// Cell content for one row.
    fn cell<'a>(&'a self, row: &'a T, row_index: usize) -> Element<'a, M>;

    /// Width specification.
    fn width(&self) -> ColumnWidth {
        ColumnWidth::Flex(1.0)
    }

    /// Minimum allowed width (for resize clamping).
    fn min_width(&self) -> f32 {
        20.0
    }

    /// Maximum width (`None` = unbounded).
    fn max_width(&self) -> Option<f32> {
        None
    }

    /// Whether this column can be resized by dragging.
    fn resizable(&self) -> bool {
        true
    }

    /// Whether clicking the header triggers sort.
    fn sortable(&self) -> bool {
        false
    }

    /// Whether this column can be reordered by header drag.
    fn reorderable(&self) -> bool {
        true
    }

    /// Compare two rows for ascending sort. Default: `Equal` (stable no-op).
    fn compare(&self, _a: &T, _b: &T) -> std::cmp::Ordering {
        std::cmp::Ordering::Equal
    }

    /// Horizontal alignment of cell content.
    fn align(&self) -> Alignment {
        Alignment::Start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_direction_toggle() {
        assert_eq!(SortDirection::Ascending.toggle(), SortDirection::Descending);
        assert_eq!(SortDirection::Descending.toggle(), SortDirection::Ascending);
    }

    #[test]
    fn sort_direction_indicator() {
        assert_eq!(SortDirection::Ascending.indicator(), " \u{25B2}");
        assert_eq!(SortDirection::Descending.indicator(), " \u{25BC}");
    }

    #[test]
    fn column_id_display() {
        let id = ColumnId("ticker");
        assert_eq!(id.to_string(), "ticker");
    }

    #[test]
    fn column_id_equality() {
        assert_eq!(ColumnId("price"), ColumnId("price"));
        assert_ne!(ColumnId("price"), ColumnId("ticker"));
    }

    #[test]
    fn column_id_copy() {
        let id = ColumnId("test");
        let id2 = id;
        assert_eq!(id, id2);
    }
}
