//! Grid chrome event type.
//!
//! `GridMessage` represents events from grid chrome (sort headers,
//! selection areas). Cell content emits the application's message
//! type `M` directly — it does not go through `GridMessage`.

use crate::column::ColumnId;

/// Messages emitted by grid chrome.
///
/// The application maps these into its own `Message` enum via the
/// `on_grid` callback provided to the grid constructor.
///
/// Phase 0 defines `SortToggled` and `RowSelected`.
/// Phase 1 adds resize variants; Phase 2 adds drag variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridMessage {
    /// User clicked a sortable column header.
    SortToggled(ColumnId),
    /// User clicked a row (single select).
    RowSelected(usize),
    // Phase 1 adds: ResizeStarted(ColumnId), Resizing(f32), ResizeEnded
    // Phase 2 adds: ColumnDragStarted, RowDragStarted, etc.
}
