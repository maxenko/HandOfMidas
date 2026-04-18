//! Orders tab inside the [`super::AccountPanel`].
//!
//! Holds the per-pane grid state (column widths, sort, selection) and
//! symbol-link mode for the shared [`crate::order_blotter::OrderBlotter`]
//! row store. Round-trips to [`midas_core::config::OrdersTabConfig`] so
//! layout and widths survive a restart.
//!
//! Mirrors the ex-`OrderBlotterPanel` struct field for field; the row
//! store itself is untouched so all 8 `OrderBlotter` state-machine
//! tests remain green.

use std::collections::HashSet;

use midas_core::config::OrdersTabConfig;
use midas_core::LinkMode;

/// Per-Account-panel view state for the Orders tab.
#[derive(Debug, Clone)]
#[allow(dead_code)] // `last_seen_generation` is reserved for a future rebuild cache.
pub struct OrdersTab {
    /// Grid state (column widths, sort, scroll) for this tab.
    pub grid_state: midas_grid::GridState,
    /// Symbol link group — row clicks broadcast the clicked order's
    /// symbol to every chart/panel sharing the same colour.
    pub symbol_link: LinkMode,
    /// Columns the user has hidden via the column-selector popup.
    /// Not-present = visible. Defaults to empty (all visible).
    pub hidden_columns: HashSet<midas_grid::ColumnId>,
    /// Most recent `OrderBlotter::generation()` the tab built its
    /// display rows against. Used by the view layer to skip work when
    /// nothing has changed.
    pub last_seen_generation: u64,
    /// Currently selected order row, keyed by broker-assigned order UUID.
    /// `None` = nothing selected. Session state only — not persisted.
    pub selected_row: Option<uuid::Uuid>,
}

impl Default for OrdersTab {
    fn default() -> Self {
        Self::new()
    }
}

impl OrdersTab {
    /// Fresh tab with default column widths and descending Order-ID sort.
    pub fn new() -> Self {
        Self {
            grid_state: default_grid_state(),
            symbol_link: LinkMode::default(),
            hidden_columns: HashSet::new(),
            last_seen_generation: 0,
            selected_row: None,
        }
    }

    /// Build the tab from a persisted `OrdersTabConfig`.
    ///
    /// Forward-compat: if the persisted `column_widths` length does not
    /// match the current schema (e.g. a column was added), falls back
    /// to defaults.
    pub fn from_config(cfg: &OrdersTabConfig) -> Self {
        let mut state = default_grid_state();
        let ids = crate::order_blotter::columns::OrderBlotterColumn::ids();
        if cfg.column_widths.len() == ids.len() {
            for (col_id, w) in ids.iter().zip(cfg.column_widths.iter()) {
                state.set_column_width(*col_id, *w, 20.0, None);
            }
        }
        let hidden_columns = cfg
            .hidden_columns
            .iter()
            .map(|s| midas_grid::ColumnId(string_to_static(s)))
            .collect();
        Self {
            grid_state: state,
            symbol_link: cfg.symbol_link,
            hidden_columns,
            last_seen_generation: 0,
            selected_row: None,
        }
    }

    /// Project back to a serialisable config for `AppConfig` round-trip.
    pub fn to_config(&self) -> OrdersTabConfig {
        let ids = crate::order_blotter::columns::OrderBlotterColumn::ids();
        let column_widths = ids
            .iter()
            .map(|id| self.grid_state.column_width(*id))
            .collect();
        let hidden_columns = self
            .hidden_columns
            .iter()
            .map(|id| id.0.to_owned())
            .collect();
        OrdersTabConfig {
            column_widths,
            symbol_link: self.symbol_link,
            hidden_columns,
        }
    }

    /// Whether a given column should be rendered.
    #[allow(dead_code)]
    pub fn column_visible(&self, id: midas_grid::ColumnId) -> bool {
        !self.hidden_columns.contains(&id)
    }
}

/// `midas_grid::ColumnId` wraps a `&'static str`. Persisting an owned
/// `String` from config requires leaking it into a static — acceptable
/// because the config schema has a bounded set of column IDs, so
/// leakage is bounded to one-per-known-id across process lifetime.
fn string_to_static(s: &str) -> &'static str {
    for known in crate::order_blotter::columns::OrderBlotterColumn::ids() {
        if known.0 == s {
            return known.0;
        }
    }
    Box::leak(s.to_owned().into_boxed_str())
}

/// Initialise a grid state with the blotter's column order + default
/// widths. Applied to every new tab; `from_config` overrides widths.
fn default_grid_state() -> midas_grid::GridState {
    use std::collections::HashMap;
    let ids = crate::order_blotter::columns::OrderBlotterColumn::ids();
    let widths: HashMap<midas_grid::ColumnId, f32> =
        crate::order_blotter::columns::OrderBlotterColumn::default_widths()
            .into_iter()
            .collect();
    let mut state = midas_grid::GridState::new(ids, widths);
    state.toggle_sort(
        crate::order_blotter::columns::COL_ORDER_ID,
        midas_grid::SortDirection::Descending,
    );
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_descending_order_id_sort() {
        let tab = OrdersTab::new();
        let sort = tab.grid_state.sort.expect("default sort set");
        assert_eq!(sort.column_id, crate::order_blotter::columns::COL_ORDER_ID);
        assert_eq!(sort.direction, midas_grid::SortDirection::Descending);
    }

    #[test]
    fn from_config_to_config_roundtrip() {
        let mut tab = OrdersTab::new();
        tab.symbol_link = LinkMode::ListenAll;
        tab.hidden_columns
            .insert(crate::order_blotter::columns::COL_TP);

        let cfg = tab.to_config();
        let restored = OrdersTab::from_config(&cfg);

        assert_eq!(restored.symbol_link, LinkMode::ListenAll);
        assert!(restored
            .hidden_columns
            .contains(&crate::order_blotter::columns::COL_TP));
    }

    #[test]
    fn from_config_ignores_mismatched_width_count() {
        let cfg = OrdersTabConfig {
            column_widths: vec![1.0, 2.0], // wrong length
            symbol_link: LinkMode::default(),
            hidden_columns: Vec::new(),
        };
        // Must not panic; must fall back to defaults.
        let tab = OrdersTab::from_config(&cfg);
        let ids = crate::order_blotter::columns::OrderBlotterColumn::ids();
        // Defaults applied — width for first column matches default.
        let defaults: std::collections::HashMap<_, _> =
            crate::order_blotter::columns::OrderBlotterColumn::default_widths()
                .into_iter()
                .collect();
        let expected = *defaults.get(&ids[0]).unwrap();
        assert_eq!(tab.grid_state.column_width(ids[0]), expected);
    }
}
