//! `OrderBlotterPanel` — the pane-grid-hosted UI that renders
//! [`super::OrderBlotter`] as a sortable grid.

use std::collections::HashSet;

use midas_core::{LinkMode, OrderBlotterId};

/// Blotter panel state. One instance per Orders pane the user opens;
/// all instances read from the single shared [`super::OrderBlotter`]
/// on `MidasApp`.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OrderBlotterPanel {
    pub id: OrderBlotterId,
    pub name: String,
    /// Grid state (column widths, sort, scroll) for this panel.
    pub grid_state: midas_grid::GridState,
    /// Symbol link group — row clicks broadcast the clicked order's
    /// symbol to every chart/panel sharing the same colour.
    pub symbol_link: LinkMode,
    /// Columns the user has hidden via the column-selector popup.
    /// Not-present = visible. Defaults to empty (all visible).
    pub hidden_columns: HashSet<midas_grid::ColumnId>,
    /// Most recent `OrderBlotter::generation()` the panel built its
    /// display rows against. Used by the view layer to skip work when
    /// nothing has changed.
    pub last_seen_generation: u64,
    /// Currently selected order row, keyed by broker-assigned order UUID.
    /// `None` = nothing selected. Session state only — not persisted.
    pub selected_row: Option<uuid::Uuid>,
}

impl OrderBlotterPanel {
    pub fn new(id: OrderBlotterId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            grid_state: default_grid_state(),
            symbol_link: LinkMode::default(),
            hidden_columns: HashSet::new(),
            last_seen_generation: 0,
            selected_row: None,
        }
    }

    /// Build the panel from a persisted `OrderBlotterConfig`. Column
    /// widths + link mode + hidden columns round-trip so user prefs
    /// survive restart.
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
        let hidden_columns = cfg
            .hidden_columns
            .iter()
            .map(|s| midas_grid::ColumnId(string_to_static(s)))
            .collect();
        Self {
            id,
            name: cfg.name.clone(),
            grid_state: state,
            symbol_link: cfg.symbol_link,
            hidden_columns,
            last_seen_generation: 0,
            selected_row: None,
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
        let hidden_columns = self
            .hidden_columns
            .iter()
            .map(|id| id.0.to_owned())
            .collect();
        midas_core::config::OrderBlotterConfig {
            name: self.name.clone(),
            column_widths,
            symbol_link: self.symbol_link,
            hidden_columns,
        }
    }

    /// Whether a given column should be rendered. Inlined in the view
    /// today; exposed for consumers that filter `ALL` elsewhere.
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
    // Match against the known column-id set; unknown ids leak a fresh
    // static (forward-compat for newer configs pointing at columns
    // this build doesn't know about yet).
    for known in super::columns::OrderBlotterColumn::ids() {
        if known.0 == s {
            return known.0;
        }
    }
    Box::leak(s.to_owned().into_boxed_str())
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
