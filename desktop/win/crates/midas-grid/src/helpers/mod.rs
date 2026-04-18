//! Stateless helpers for hand-built panels.
//!
//! These helpers take owned / prebuilt `M`-typed messages and return
//! `Element<'_, M>` trees for per-cell / per-row composition. They are
//! the complement to the legacy whole-row `grid_header` / `grid_body`
//! helpers (used by the [`crate::widget::grid`] builder) — panels that
//! need to hand-roll their table chrome should compose these helpers
//! per-column and per-row instead of calling `grid()`.
//!
//! # Why two sets?
//!
//! The `grid()` builder wants `&'a [T]` for locally-built rows, which
//! doesn't work when the `Vec<Row>` is built inside a `view()` fn (the
//! borrow can't escape). These helpers side-step that by taking the
//! caller's already-constructed `Vec<Element<'a, M>>` cells directly.

pub mod body_cell;
pub mod body_row;
pub mod header_cell;
pub mod popup;

pub use body_cell::{grid_body_cell, BODY_CELL_PADDING};
pub use body_row::grid_body_row;
pub use header_cell::{grid_header_cell, HeaderStyle};
pub use popup::{column_selector_popup, ColumnEntry};

/// Drag-handle descriptor for a resizable column divider.
///
/// Passed into `grid_header_cell` to layer a 4-px right-edge strip on
/// top of the header content via `stack!`. The strip emits `on_press`
/// when the user mouses-down on it; the caller is responsible for the
/// actual drag / release overlay.
///
/// The hit-strip width is always 4 logical px (baked in to match the
/// historical inline behaviour); only `height` is configurable.
#[derive(Debug, Clone)]
pub struct ResizeHandle<M: Clone> {
    /// Message emitted on mouse-press on the handle strip.
    pub on_press: M,
    /// Height of the hit strip, logical px.
    pub height: f32,
}
