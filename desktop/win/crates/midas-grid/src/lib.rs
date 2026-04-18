//! `midas-grid` — Headless, trait-based grid widget for Hand of Midas.
//!
//! A reusable grid/table component built on iced 0.14. The grid separates
//! state management from rendering (headless core pattern). It never owns,
//! sorts, or filters data — it holds UI state and emits intents.
//!
//! # Architecture
//!
//! - **`GridState`** is a plain struct owned by the application.
//! - **`GridColumn<T, M>`** trait defines how columns render.
//! - **`GridMessage`** carries grid chrome events; cells emit `M` directly.
//! - **`grid()`** composes header + scrollable body into an `Element<M>`.

pub mod body;
pub mod column;
pub mod columns;
pub mod header;
pub mod helpers;
pub mod message;
pub mod state;
pub mod style;
pub mod widget;

// Re-exports for convenient access.
pub use column::{Alignment, ColumnId, ColumnWidth, GridColumn, SortDirection, SortSpec};
pub use helpers::{
    column_selector_popup, grid_body_cell, grid_body_row, grid_header_cell, ColumnEntry,
    HeaderStyle, ResizeHandle, BODY_CELL_PADDING,
};
pub use message::GridMessage;
pub use state::{ActiveInteraction, GridState, SelectionState};
pub use style::{GridStyle, ALT_ROW_BG, GRID_BORDER_COLOR, GRID_HEADER_BORDER_COLOR};
pub use widget::grid;
