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
    // Grid state should have default widths.
    let defaults = default_column_widths();
    for (&id, &expected) in defaults.iter() {
        assert_eq!(wl.grid_state.column_width(id), expected);
    }
}

#[test]
fn column_widths_roundtrip() {
    let mut wl = WatchlistPanel::new(WatchlistId::new(1), "Test".into());
    wl.grid_state.set_column_width(COL_TICKER, 120.0, 20.0, None); // widen ticker column
    let config = wl.to_config();
    let restored = WatchlistPanel::from_config(WatchlistId::new(2), &config);
    assert_eq!(restored.grid_state.column_width(COL_TICKER), 120.0);
    assert_eq!(
        restored.grid_state.column_width(COL_DRAG),
        default_column_widths()[&COL_DRAG],
    ); // unchanged
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
    for &id in &WATCHLIST_COLUMN_ORDER {
        let w = wl.grid_state.column_width(id);
        assert!(w >= 20.0, "column width {w} for {id} should be >= 20.0");
    }
}
