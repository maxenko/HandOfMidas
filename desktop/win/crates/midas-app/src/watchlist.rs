//! Watchlist panel state and operations.
//!
//! A watchlist is an editable list of ticker symbols that lives alongside
//! chart panels in the workspace pane grid. It persists across sessions
//! via the TOML config file.

use midas_core::config::{WatchlistConfig, WatchlistTickerConfig};
use midas_core::{LinkMode, WatchlistId};

// ── Per-ticker data ─────────────────────────────────────────────────

/// A single ticker entry within a watchlist.
#[derive(Debug, Clone)]
pub struct WatchlistTicker {
    /// Ticker symbol, always uppercase (e.g. `"AAPL"`).
    pub symbol: String,
    /// Whether this ticker is marked as a favorite.
    pub favorite: bool,
}

// ── Watchlist panel ─────────────────────────────────────────────────

/// Runtime state for one watchlist panel.
#[derive(Debug, Clone)]
pub struct WatchlistPanel {
    /// Unique identifier for this watchlist within the workspace.
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
    /// Column widths in logical pixels.
    pub column_widths: [f32; 7],
}

/// Default column widths: [drag, fav, ticker, price, chg%, G.ATR, delete].
pub const DEFAULT_COLUMN_WIDTHS: [f32; 7] = [26.0, 30.0, 70.0, 80.0, 65.0, 70.0, 30.0];

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
            column_widths: DEFAULT_COLUMN_WIDTHS,
        }
    }

    /// Restore a watchlist from persisted config.
    pub fn from_config(id: WatchlistId, config: &WatchlistConfig) -> Self {
        let column_widths = if config.column_widths.len() == 7 {
            let mut arr = DEFAULT_COLUMN_WIDTHS;
            for (i, &w) in config.column_widths.iter().enumerate() {
                arr[i] = w.max(20.0);
            }
            arr
        } else {
            DEFAULT_COLUMN_WIDTHS
        };
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
            column_widths,
        }
    }

    /// Serialize this watchlist to a config struct for persistence.
    pub fn to_config(&self) -> WatchlistConfig {
        WatchlistConfig {
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
            column_widths: self.column_widths.to_vec(),
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
            favorite: false,
        });
        true
    }

    /// Remove a ticker by symbol (case-insensitive).
    pub fn remove_ticker(&mut self, symbol: &str) {
        let upper = symbol.to_uppercase();
        self.tickers.retain(|t| t.symbol != upper);
    }

    /// Toggle the favorite status of a ticker (case-insensitive).
    pub fn toggle_favorite(&mut self, symbol: &str) {
        let upper = symbol.to_uppercase();
        if let Some(t) = self.tickers.iter_mut().find(|t| t.symbol == upper) {
            t.favorite = !t.favorite;
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
mod tests {
    use super::*;

    #[test]
    fn new_watchlist_is_empty() {
        let wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        assert!(wl.tickers.is_empty());
        assert_eq!(wl.name, "Test");
        assert!(wl.add_ticker_input.is_empty());
    }

    #[test]
    fn add_ticker_normalizes_to_uppercase() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        assert!(wl.add_ticker("aapl"));
        assert_eq!(wl.tickers[0].symbol, "AAPL");
    }

    #[test]
    fn add_ticker_trims_whitespace() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        assert!(wl.add_ticker("  msft  "));
        assert_eq!(wl.tickers[0].symbol, "MSFT");
    }

    #[test]
    fn add_ticker_rejects_empty() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        assert!(!wl.add_ticker(""));
        assert!(!wl.add_ticker("   "));
        assert!(wl.tickers.is_empty());
    }

    #[test]
    fn add_ticker_rejects_duplicate() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        assert!(wl.add_ticker("AAPL"));
        assert!(!wl.add_ticker("aapl"));
        assert!(!wl.add_ticker("Aapl"));
        assert_eq!(wl.tickers.len(), 1);
    }

    #[test]
    fn remove_ticker_case_insensitive() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        wl.add_ticker("AAPL");
        wl.add_ticker("MSFT");
        wl.remove_ticker("aapl");
        assert_eq!(wl.tickers.len(), 1);
        assert_eq!(wl.tickers[0].symbol, "MSFT");
    }

    #[test]
    fn toggle_favorite() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        wl.add_ticker("AAPL");
        assert!(!wl.tickers[0].favorite);
        wl.toggle_favorite("aapl");
        assert!(wl.tickers[0].favorite);
        wl.toggle_favorite("AAPL");
        assert!(!wl.tickers[0].favorite);
    }

    #[test]
    fn has_ticker_case_insensitive() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        wl.add_ticker("AAPL");
        assert!(wl.has_ticker("AAPL"));
        assert!(wl.has_ticker("aapl"));
        assert!(wl.has_ticker("Aapl"));
        assert!(!wl.has_ticker("MSFT"));
    }

    #[test]
    fn config_roundtrip() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Main".into());
        wl.add_ticker("AAPL");
        wl.add_ticker("MSFT");
        wl.toggle_favorite("AAPL");

        let config = wl.to_config();
        let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);

        assert_eq!(restored.name, "Main");
        assert_eq!(restored.tickers.len(), 2);
        assert_eq!(restored.tickers[0].symbol, "AAPL");
        assert!(restored.tickers[0].favorite);
        assert_eq!(restored.tickers[1].symbol, "MSFT");
        assert!(!restored.tickers[1].favorite);
        // Transient state is not persisted.
        assert!(restored.add_ticker_input.is_empty());
        assert!(restored.selected_symbol.is_none());
    }

    #[test]
    fn symbol_link_roundtrip() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Linked".into());
        wl.symbol_link = LinkMode::Color(midas_core::LinkColor::Blue);
        let config = wl.to_config();
        let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);
        assert_eq!(restored.symbol_link, LinkMode::Color(midas_core::LinkColor::Blue));
    }

    #[test]
    fn from_config_empty() {
        let config = WatchlistConfig {
            name: "Empty".into(),
            tickers: Vec::new(),
            symbol_link: LinkMode::Unlinked,
            column_widths: vec![],
        };
        let wl = WatchlistPanel::from_config(WatchlistId::new(1), &config);
        assert_eq!(wl.name, "Empty");
        assert!(wl.tickers.is_empty());
        assert_eq!(wl.column_widths, DEFAULT_COLUMN_WIDTHS);
    }

    #[test]
    fn column_widths_roundtrip() {
        let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
        wl.column_widths[2] = 120.0; // widen ticker column
        let config = wl.to_config();
        let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);
        assert_eq!(restored.column_widths[2], 120.0);
        assert_eq!(restored.column_widths[0], DEFAULT_COLUMN_WIDTHS[0]); // unchanged
    }

    #[test]
    fn column_widths_minimum_enforced() {
        let config = WatchlistConfig {
            name: "Narrow".into(),
            tickers: vec![],
            symbol_link: LinkMode::Unlinked,
            column_widths: vec![5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0],
        };
        let wl = WatchlistPanel::from_config(WatchlistId::new(1), &config);
        for &w in &wl.column_widths {
            assert!(w >= 20.0, "column width {w} should be >= 20.0");
        }
    }
}
