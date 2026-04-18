//! `OrderBlotterPanel` — the pane-grid-hosted UI that renders
//! [`super::OrderBlotter`] as a sortable grid.
//!
//! Slice 3 lands the skeleton (empty placeholder); Slice 4 wires the
//! grid columns. See `plan/order-store.md`.

use midas_core::OrderBlotterId;

/// Blotter panel state. One instance per Orders pane the user opens;
/// all instances read from the single shared [`super::OrderBlotter`]
/// on `MidasApp`.
///
/// `id`, `grid_state`, and `last_seen_generation` are reserved for the
/// Slice-4 grid rendering; the Slice-3 placeholder only reads `name`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OrderBlotterPanel {
    pub id: OrderBlotterId,
    pub name: String,
    /// Grid state (column widths, sort, scroll) for this panel.
    pub grid_state: midas_grid::GridState,
    /// Most recent `OrderBlotter::generation()` the panel built its
    /// display rows against. Used by the view layer to skip work when
    /// nothing has changed.
    pub last_seen_generation: u64,
}

impl OrderBlotterPanel {
    pub fn new(id: OrderBlotterId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            grid_state: default_grid_state(),
            last_seen_generation: 0,
        }
    }

    /// Build the panel from a persisted `OrderBlotterConfig`. Column
    /// widths round-trip through config so user resizes survive restart.
    pub fn from_config(id: OrderBlotterId, cfg: &midas_core::config::OrderBlotterConfig) -> Self {
        let mut state = default_grid_state();
        // Config stores widths in column-id order. If the count matches
        // the current schema, apply them; otherwise fall back to
        // defaults (forward-compat when we add columns).
        let ids = super::columns::OrderBlotterColumn::ids();
        if cfg.column_widths.len() == ids.len() {
            for (col_id, w) in ids.iter().zip(cfg.column_widths.iter()) {
                state.set_column_width(*col_id, *w, 20.0, None);
            }
        }
        Self {
            id,
            name: cfg.name.clone(),
            grid_state: state,
            last_seen_generation: 0,
        }
    }

    /// Project the panel back into a serialisable config for
    /// round-tripping through `AppConfig`.
    pub fn to_config(&self) -> midas_core::config::OrderBlotterConfig {
        let ids = super::columns::OrderBlotterColumn::ids();
        let column_widths = ids
            .iter()
            .map(|id| self.grid_state.column_width(*id))
            .collect();
        midas_core::config::OrderBlotterConfig {
            name: self.name.clone(),
            column_widths,
        }
    }
}

/// Initialise a grid state with the blotter's column order + default
/// widths. Applied to every new panel; `from_config` overrides widths.
fn default_grid_state() -> midas_grid::GridState {
    use std::collections::HashMap;
    let ids = super::columns::OrderBlotterColumn::ids();
    let widths: HashMap<midas_grid::ColumnId, f32> =
        super::columns::OrderBlotterColumn::default_widths()
            .into_iter()
            .collect();
    let mut state = midas_grid::GridState::new(ids, widths);
    // Default sort: Order ID descending (most recent first).
    state.toggle_sort(
        super::columns::COL_ORDER_ID,
        midas_grid::SortDirection::Descending,
    );
    state
}
