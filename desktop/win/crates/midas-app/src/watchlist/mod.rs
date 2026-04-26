//! Watchlist panel state and operations.
//!
//! A watchlist is an editable list of ticker symbols that lives alongside
//! chart panels in the workspace pane grid. It persists across sessions
//! via the TOML config file.

use std::collections::HashMap;

use midas_core::config::{WatchlistConfig, WatchlistTickerConfig};
use midas_core::{LinkMode, WatchlistId};

// ── Column ID constants ────────────────────────────────────────────

/// Column ID for the drag grip column.
pub const COL_DRAG: midas_grid::ColumnId = midas_grid::ColumnId("drag");
/// Column ID for the favorite star column.
pub const COL_FAV: midas_grid::ColumnId = midas_grid::ColumnId("fav");
/// Column ID for the ticker symbol column.
pub const COL_TICKER: midas_grid::ColumnId = midas_grid::ColumnId("ticker");
/// Column ID for the last price column.
pub const COL_PRICE: midas_grid::ColumnId = midas_grid::ColumnId("price");
/// Column ID for the change-percent column.
pub const COL_CHANGE: midas_grid::ColumnId = midas_grid::ColumnId("change");
/// Column ID for the GATR column.
pub const COL_GATR: midas_grid::ColumnId = midas_grid::ColumnId("gatr");
/// Column ID for the inline chart thumbnail column.
pub const COL_CHART: midas_grid::ColumnId = midas_grid::ColumnId("chart");
/// Column ID for the delete button column.
pub const COL_DELETE: midas_grid::ColumnId = midas_grid::ColumnId("delete");

/// Default column order for the watchlist grid.
pub const WATCHLIST_COLUMN_ORDER: [midas_grid::ColumnId; 8] = [
    COL_DRAG, COL_FAV, COL_TICKER, COL_PRICE, COL_CHANGE, COL_GATR, COL_CHART, COL_DELETE,
];

/// Build the default column widths for the watchlist grid.
pub fn default_column_widths() -> HashMap<midas_grid::ColumnId, f32> {
    let mut m = HashMap::new();
    m.insert(COL_DRAG, 26.0);
    m.insert(COL_FAV, 34.0);
    m.insert(COL_TICKER, 70.0);
    m.insert(COL_PRICE, 80.0);
    m.insert(COL_CHANGE, 65.0);
    m.insert(COL_GATR, 70.0);
    m.insert(COL_CHART, 120.0);
    m.insert(COL_DELETE, 30.0);
    m
}

// ── Per-ticker data ─────────────────────────────────────────────────

/// A single ticker entry within a watchlist.
#[derive(Debug, Clone)]
pub struct WatchlistTicker {
    /// Ticker symbol, always uppercase (e.g. `"AAPL"`).
    pub symbol: String,
    /// Favourite level: `0` = off, `1..=5` = graded silver→gold.
    ///
    /// Click cycles through `0 → 1 → 2 → 3 → 4 → 5 → 0`. Higher levels
    /// sort first so pinning a ticker with a higher level keeps it at
    /// the top of the list.
    pub favorite: u8,
}

// ── Watchlist panel ─────────────────────────────────────────────────

/// Runtime state for one watchlist panel.
#[derive(Debug, Clone)]
pub struct WatchlistPanel {
    /// Unique identifier for this watchlist within the workspace.
    #[allow(dead_code)] // part of planned API
    pub id: WatchlistId,
    /// User-visible name (e.g. `"Main"`).
    pub name: String,
    /// Ordered list of tickers in the watchlist.
    pub tickers: Vec<WatchlistTicker>,
    /// Text in the "add ticker" input field (transient, not persisted).
    pub add_ticker_input: String,
    /// Currently selected ticker symbol (transient, not persisted).
    pub selected_symbol: Option<String>,
    /// Symbol link group for watchlist→chart symbol propagation.
    pub symbol_link: LinkMode,
    /// Grid UI state (column widths, sort, selection, scroll).
    pub grid_state: midas_grid::GridState,
    /// Snapshot of favorite values used as the sort key while the
    /// cursor is hovering a favourite cell. `None` outside hover —
    /// the VM uses live `WatchlistTicker::favorite` for sorting.
    /// `Some` while hovering — pins the favourites-first sort key
    /// so multiple wheel adjustments don't reorder rows under the
    /// cursor. Cleared on `mouse_area::on_exit`. Transient state.
    pub sort_freeze: Option<HashMap<String, u8>>,
}

impl WatchlistPanel {
    /// Create an empty watchlist with the given ID and name.
    pub fn new(id: WatchlistId, name: String) -> Self {
        Self {
            id,
            name,
            tickers: Vec::new(),
            add_ticker_input: String::new(),
            selected_symbol: None,
            symbol_link: LinkMode::Unlinked,
            grid_state: midas_grid::GridState::new(
                WATCHLIST_COLUMN_ORDER.to_vec(),
                default_column_widths(),
            ),
            sort_freeze: None,
        }
    }

    /// Restore a watchlist from persisted config.
    pub fn from_config(id: WatchlistId, config: &WatchlistConfig) -> Self {
        let mut widths = default_column_widths();
        // Apply every saved width whose index lines up with the current
        // column order. Configs saved before the Chart column landed are
        // 7-wide and resolve through the same loop against the current
        // 8-wide `WATCHLIST_COLUMN_ORDER` — any missing trailing entries
        // keep their default width from `default_column_widths()`.
        let ids = WATCHLIST_COLUMN_ORDER;
        for (i, &w) in config.column_widths.iter().take(ids.len()).enumerate() {
            // COL_FAV is non-resizable and its default width is tuned
            // to the current star size — ignore the saved value so
            // older configs don't squeeze the glyph into a narrow
            // column.
            if ids[i] == COL_FAV {
                continue;
            }
            widths.insert(ids[i], w.max(20.0));
        }
        Self {
            id,
            name: config.name.clone(),
            tickers: config
                .tickers
                .iter()
                .map(|t| WatchlistTicker {
                    symbol: t.symbol.clone(),
                    favorite: t.favorite,
                })
                .collect(),
            add_ticker_input: String::new(),
            selected_symbol: None,
            symbol_link: config.symbol_link,
            grid_state: midas_grid::GridState::new(WATCHLIST_COLUMN_ORDER.to_vec(), widths),
            sort_freeze: None,
        }
    }

    /// Serialize this watchlist to a config struct for persistence.
    pub fn to_config(&self) -> WatchlistConfig {
        WatchlistConfig {
            id: self.id.0,
            name: self.name.clone(),
            tickers: self
                .tickers
                .iter()
                .map(|t| WatchlistTickerConfig {
                    symbol: t.symbol.clone(),
                    favorite: t.favorite,
                })
                .collect(),
            symbol_link: self.symbol_link,
            column_widths: WATCHLIST_COLUMN_ORDER
                .iter()
                .map(|id| self.grid_state.column_width(*id))
                .collect(),
        }
    }

    /// Add a ticker to the watchlist.
    ///
    /// Normalizes to uppercase and trims whitespace. Returns `false` if
    /// the symbol is empty after trimming or already present (case-insensitive).
    pub fn add_ticker(&mut self, symbol: &str) -> bool {
        let normalized = symbol.trim().to_uppercase();
        if normalized.is_empty() {
            return false;
        }
        if self.has_ticker(&normalized) {
            return false;
        }
        self.tickers.push(WatchlistTicker {
            symbol: normalized,
            favorite: 0,
        });
        true
    }

    /// Remove a ticker by symbol (case-insensitive).
    pub fn remove_ticker(&mut self, symbol: &str) {
        let upper = symbol.to_uppercase();
        self.tickers.retain(|t| t.symbol != upper);
    }

    /// Snapshot the current per-symbol favourite values into
    /// [`Self::sort_freeze`]. Called from the favourite cell's
    /// `mouse_area::on_enter` so subsequent wheel adjustments don't
    /// reorder the row under the cursor. No-op if a freeze is
    /// already active — back-to-back `on_enter` events (e.g. from
    /// rapid mouse movement) keep the original snapshot.
    pub fn freeze_sort(&mut self) {
        if self.sort_freeze.is_some() {
            return;
        }
        self.sort_freeze = Some(
            self.tickers
                .iter()
                .map(|t| (t.symbol.clone(), t.favorite))
                .collect(),
        );
    }

    /// Release the favourites-first sort snapshot. Called from the
    /// favourite cell's `mouse_area::on_exit` — the next render
    /// re-sorts using live `favorite` values.
    pub fn unfreeze_sort(&mut self) {
        self.sort_freeze = None;
    }

    /// Adjust the favourite level of a ticker by `delta`, clamped to `0..=5`.
    ///
    /// Driven by the scroll wheel over the favourite column: `+1` per
    /// line up, `-1` per line down. A no-op if the symbol is unknown
    /// or the clamped value is unchanged.
    pub fn adjust_favorite(&mut self, symbol: &str, delta: i8) {
        let upper = symbol.to_uppercase();
        if let Some(t) = self.tickers.iter_mut().find(|t| t.symbol == upper) {
            let next = (t.favorite as i16 + delta as i16).clamp(0, 5) as u8;
            t.favorite = next;
        }
    }

    /// Check if the watchlist contains the given symbol (case-insensitive).
    pub fn has_ticker(&self, symbol: &str) -> bool {
        let upper = symbol.to_uppercase();
        self.tickers.iter().any(|t| t.symbol == upper)
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
